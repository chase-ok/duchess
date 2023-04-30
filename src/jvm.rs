use crate::{
    cast::{AsUpcast, TryDowncast, Upcast},
    catch::{CatchNone, Catching},
    error::check_exception,
    inspect::{ArgOp, Inspect},
    java::lang::{Class, ClassExt, Throwable},
    not_null::NotNull,
    plumbing::{convert_non_throw_jni_error},
    Error, IntoLocal,
};

use std::{
    env,
    ffi::CStr,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use jni::{
    objects::{AutoLocal, GlobalRef, JObject, JValueOwned},
    InitArgsBuilder, JNIEnv, JavaVM,
};
use once_cell::sync::{Lazy, OnceCell};

/// A "jdk op" is a suspended operation that, when executed, will run
/// on the jvm, producing a value of type `Output`. These ops typically
/// represent constructor or method calls, and they can be chained
/// together.
///
/// *Eventual goal:* Each call to `execute` represents a single crossing
/// over into the JVM, so the more you can chain together your jvm-ops,
/// the better.
pub trait JvmOp: Sized {
    type Input<'jvm>;
    type Output<'jvm>;

    /// "Inspect" executes an operation on this value that results in unit
    /// and then yields up this value again for further use. If the result
    /// of `self` is the java value `x`, then `self.inspect(|x| x.foo()).bar()`
    /// is equivalent to the Java code `x.foo(); return x.bar()`.
    fn inspect<K>(self, op: impl FnOnce(ArgOp<Self>) -> K) -> Inspect<Self, K>
    where
        for<'jvm> Self::Output<'jvm>: CloneIn<'jvm>,
        K: JvmOp,
        for<'jvm> K: JvmOp<Input<'jvm> = Self::Output<'jvm>, Output<'jvm> = ()>,
    {
        Inspect::new(self, op)
    }

    /// Start a set of catch blocks that can handle exceptions thrown by `self`. Multiple
    /// blocks can be added via [`Catching::catch`] for different exception classes, as well as
    /// a finally block.
    fn catching(self) -> Catching<Self, CatchNone> {
        Catching::new(self)
    }

    fn assert_not_null<T>(self) -> NotNull<Self>
    where
        T: JavaObject,
        for<'jvm> Self: JvmOp<Output<'jvm> = Option<Local<'jvm, T>>>,
    {
        NotNull::new(self)
    }

    /// Tries to downcast output of this operation to `To`, otherwise returning
    /// the output as is. Equivalent to
    /// ```java
    /// From x;
    /// if (x instanceof To) {
    ///    return Ok((To) x);
    /// } else {
    ///    return Err(x);
    /// }
    /// ```
    fn try_downcast<From, To>(self) -> TryDowncast<Self, From, To>
    where
        for<'jvm> Self::Output<'jvm>: AsRef<From>,
        From: JavaObject,
        To: Upcast<From>,
    {
        TryDowncast::new(self)
    }

    /// Most duchess-wrapped Java objects will automatically be able to call all
    /// methods defined on any of its super classes or interfaces it implements,
    /// but this can be used to "force" the output of the operation to be typed
    /// as an explicit super type `To`.
    fn upcast<From, To>(self) -> AsUpcast<Self, From, To>
    where
        for<'jvm> Self::Output<'jvm>: AsRef<From>,
        From: Upcast<To>,
        To: JavaObject,
    {
        AsUpcast::new(self)
    }

    fn execute_with<'jvm>(
        self,
        jvm: &mut Jvm<'jvm>,
        arg: Self::Input<'jvm>,
    ) -> crate::Result<'jvm, Self::Output<'jvm>>;
}

/// This trait is only implemented for `()`; it allows the `JvmOp::execute` method to only
/// be used for `()`.
pub trait IsVoid: Default {}
impl IsVoid for () {}

static GLOBAL_JVM: Lazy<JavaVM> = Lazy::new(|| {
    let mut jvm_builder = InitArgsBuilder::new()
        .version(jni::JNIVersion::V8)
        .option("-Xcheck:jni");
    if let Ok(classpath) = env::var("CLASSPATH") {
        jvm_builder = jvm_builder.option(format!("-Djava.class.path={classpath}"));
    }
    let jvm_args = jvm_builder.build().unwrap();

    JavaVM::new(jvm_args).unwrap()
});

#[repr(transparent)]
pub struct Jvm<'jvm> {
    env: JNIEnv<'jvm>,
}

impl<'jvm> Jvm<'jvm> {
    pub fn with<R>(
        op: impl for<'a> FnOnce(&mut Jvm<'a>) -> crate::Result<'a, R>,
    ) -> crate::GlobalResult<R> {
        let guard = GLOBAL_JVM
            .attach_current_thread()
            .map_err(convert_non_throw_jni_error)?;

        // Safety condition: must not be used to create new references
        // unless they are contained by `guard`. In this case, the
        // cloned env is fully contained within the lifetime of `guard`
        // and basically takes its place. The only purpose here is to
        // avoid having two lifetime parameters on `Jvm`; trying to
        // keep the interface simpler.
        let env = unsafe { guard.unsafe_clone() };

        op(&mut Jvm { env }).map_err(|e| {
            e.into_global(&mut Jvm {
                env: unsafe { guard.unsafe_clone() },
            })
        })
    }

    pub fn to_env(&mut self) -> &mut JNIEnv<'jvm> {
        &mut self.env
    }

    /// XX
    pub fn as_raw(&self) -> *mut jni_sys::JNIEnv {
        self.env.get_raw()
    }

    pub fn local<R>(&mut self, r: &R) -> Local<'jvm, R>
    where
        R: JavaObject,
    {
        let jni = self.as_raw();
        let obj = r.as_raw();
        // XX: safety of lifetime
        unsafe {
            let new_ref = (**jni).NewLocalRef.unwrap()(jni, obj.as_ptr());
            // XX: new_ref may be nul if we ran out of memory
            Local::from_raw(jni, NonNull::new(new_ref).unwrap())
        }
    }

    pub fn global<R>(&mut self, r: &R) -> Global<R>
    where
        R: JavaObject,
    {
        let jni = self.as_raw();
        let obj = r.as_raw();
        unsafe {
            let new_ref = (**jni).NewGlobalRef.unwrap()(jni, obj.as_ptr());
            // XX: new_ref may be nul if we ran out of memory
            Global::from_raw(NonNull::new(new_ref).unwrap())
        }
    }
}

/// A trait for zero-sized dummy types that represent Java object types.
///
/// # Safety
///
/// A type `T` that implements this trait must satisfy the following contract:
///
/// 1. `T` must be a zero-sized type.
/// 2. It must not be possible to construct a value of type `T`.
/// 3. The alignment of `T` must *not* be greater than the alignment of [jni::sys::_jobject]. (I
///    *think* this is always true for zero-sized types, so would be implied by rule #1, but I'm not
///    sure.)
///
/// # Example
///
/// ```ignore
/// # use duchess::JavaObject;
/// pub struct BigDecimal {
///     _private: (), // prevent construction
/// }
/// unsafe impl JavaObject for BigDecimal {}
/// ```
pub unsafe trait JavaObject: 'static + Sized + JavaType {
    // XX: can't be put on extension trait nor define a default because we want to cache the resolved
    // class in a static OnceCell.
    /// Returns Java Class object for this type.
    fn class<'jvm>(jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, Local<'jvm, Class>>;
}

/// Extension trait for [JavaObject].
pub trait JavaObjectExt: Sized {
    // We use an extension trait, instead of just declaring these functions on the main JavaObject
    // trait, to prevent trait implementors from overriding the implementation of these functions.

    fn as_jobject(&self) -> BorrowedJObject<'_>;

    unsafe fn from_raw<'a>(ptr: NonNull<jni_sys::_jobject>) -> &'a Self;
    fn as_raw(&self) -> NonNull<jni_sys::_jobject>;
}
impl<T: JavaObject> JavaObjectExt for T {
    fn as_jobject(&self) -> BorrowedJObject<'_> {
        let raw = self.as_raw();

        // SAFETY: the only way to get a `&Self` is by calling `Self::from_jobject` (trait rule #1),
        // so reconstructing the original JObject passed to `from_jni` should also be safe.
        let obj = unsafe { JObject::from_raw(raw.as_ptr()) };

        // We must wrap the JObject to prevent anyone from calling `delete_local_ref` on it;
        // otherwise, `self` could become dangling
        BorrowedJObject::new(obj)
    }

    unsafe fn from_raw<'a>(ptr: NonNull<jni_sys::_jobject>) -> &'a Self {
        // SAFETY: I *think* the cast is sound, because:
        //
        // 1. A pointer to a suitably aligned `sys::_jobject` should also satisfy Self's alignment
        //    requirement (trait rule #3)
        // 2. Self is a zero-sized type (trait rule #1), so there are no invalid bit patterns to
        //    worry about.
        // 3. Self is a zero-sized type (trait rule #1), so there's no actual memory region that is
        //    subject to the aliasing rules.
        //
        // XXX: Please check my homework.
        unsafe { ptr.cast().as_ref() }
    }

    fn as_raw(&self) -> NonNull<jni_sys::_jobject> {
        // XX: safety
        unsafe {
            NonNull::new_unchecked((self as *const Self).cast_mut()).cast::<jni_sys::_jobject>()
        }
    }
}

pub unsafe trait JavaType: 'static {
    /// Returns the Java Class object for a Java array containing elements of
    /// `Self`. All Java types, even scalars can be elements of an array object.
    fn array_class<'jvm>(jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, Local<'jvm, Class>>;
}

unsafe impl<T: JavaObject> JavaType for T {
    fn array_class<'jvm>(jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, Local<'jvm, Class>> {
        T::class(jvm)?.array_type().assert_not_null().execute(jvm)
    }
}

pub trait JavaScalar: JavaType {}

macro_rules! scalar {
    ($($rust:ty: $array_class:literal,)*) => {
        $(
            unsafe impl JavaType for $rust {
                fn array_class<'jvm>(jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, Local<'jvm, Class>> {
                    let jni = jvm.as_raw();

                    // XX: Safety
                    const CLASS_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked($array_class) };
                    static CLASS: OnceCell<Global<crate::java::lang::Class>> = OnceCell::new();

                    let global = CLASS.get_or_try_init::<_, crate::Error<Local<Throwable>>>(|| {
                        // XX: safety
                        let class = unsafe { (**jni).FindClass.unwrap()(jni, CLASS_NAME.as_ptr()) };
                        if let Some(class) = NonNull::new(class) {
                            Ok(jvm.global(unsafe { &Local::from_raw(jni, class) }))
                        } else {
                            check_exception(jvm)?;
                            Err(Error::JvmInternal(format!("failed to load array class `{}`", CLASS_NAME.to_string_lossy())))
                        }
                    })?;
                    Ok(jvm.local(global))
                }
            }

            impl JavaScalar for $rust {}
        )*
    };
}

scalar! {
    bool: b"[Z\0",
    i8:   b"[B\0",
    i16:  b"[S\0",
    u16:  b"[C\0",
    i32:  b"[I\0",
    i64:  b"[J\0",
    f32:  b"[F\0",
    f64:  b"[D\0",
}

/// A wrapper for a [JObject] that only allows access by reference. This prevents passing the
/// wrapped `JObject` to `JNIEnv::delete_local_ref`.
pub type BorrowedJObject<'a> = Jail<JObject<'a>>;

/// A wrapper for a value that prevents the value from being moved out, while still allowing access
/// by reference.
pub struct Jail<T>(T);

impl<T> Jail<T> {
    pub fn new(value: T) -> Self {
        Self(value)
    }
}
impl<T> Deref for Jail<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> DerefMut for Jail<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<T> AsRef<T> for Jail<T> {
    fn as_ref(&self) -> &T {
        &*self
    }
}
impl<T> AsMut<T> for Jail<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut *self
    }
}

/// An owned local reference to a non-null Java object of type `T`. The reference will be freed when
/// dropped.
pub struct Local<'a, T: JavaObject> {
    ptr: NonNull<jni_sys::_jobject>,
    jni: *mut jni_sys::JNIEnv,
    _marker: PhantomData<&'a T>,
}

// XX: safety
unsafe impl<T: JavaObject> Send for Local<'_, T> {}
// XX: safety
unsafe impl<T: JavaObject> Sync for Local<'_, T> {}

impl<T: JavaObject> Drop for Local<'_, T> {
    fn drop(&mut self) {
        let jni = self.jni;
        // XX safety
        unsafe { (**jni).DeleteLocalRef.unwrap()(jni, self.ptr.as_ptr()) }
    }
}

impl<T: JavaObject> Deref for Local<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // XX: safety
        unsafe { T::from_raw(self.ptr) }
    }
}

impl<'a, T: JavaObject> Local<'a, T> {
    pub unsafe fn from_raw(jni: *mut jni_sys::JNIEnv, ptr: NonNull<jni_sys::_jobject>) -> Self {
        Self {
            ptr,
            jni,
            _marker: PhantomData,
        }
    }

    pub unsafe fn from_jni(_inner: AutoLocal<'a, JObject<'a>>) -> Self {
        todo!()
    }
}

/// An owned global reference to a non-null Java object of type `T`. The reference will be freed
/// when dropped.
/// XX: not wrapped in Arc like jni
pub struct Global<T: JavaObject> {
    ptr: NonNull<jni_sys::_jobject>,
    _marker: PhantomData<T>,
}

// XX: safety
unsafe impl<T: JavaObject> Send for Global<T> {}
// XX: safety
unsafe impl<T: JavaObject> Sync for Global<T> {}

impl<T: JavaObject> Drop for Global<T> {
    fn drop(&mut self) {
        let result = Jvm::with(|jvm| {
            let raw = jvm.as_raw();
            // XX: safety
            unsafe {
                (**raw).DeleteGlobalRef.unwrap()(raw, self.ptr.as_ptr());
            }
            Ok(())
        });
        if let Err(_e) = result {
            // XX debug log
        }
    }
}

impl<T: JavaObject> Deref for Global<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // XX: safety
        unsafe { T::from_raw(self.ptr) }
    }
}

impl<T: JavaObject> Global<T> {
    unsafe fn from_raw(ptr: NonNull<jni_sys::_jobject>) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
    }

    pub unsafe fn from_jni(inner: GlobalRef) -> Self {
        Self::from_raw(NonNull::new(inner.as_raw()).unwrap())
    }
}

impl<'a, R, S> AsRef<S> for Local<'a, R>
where
    R: Upcast<S>,
    S: JavaObject + 'a,
{
    fn as_ref(&self) -> &S {
        // XX: Safety
        unsafe { S::from_raw(self.ptr) }
    }
}

impl<'a, R: JavaObject> Local<'a, R> {
    /// XX: Trying to map onto Into causes impl conflits
    pub fn upcast<S>(self) -> Local<'a, S>
    where
        R: Upcast<S>,
        S: JavaObject + 'a,
    {
        // XX: safety
        let upcast = unsafe { Local::<S>::from_raw(self.jni, self.ptr) };
        upcast
    }
}

impl<R, S> AsRef<S> for Global<R>
where
    R: Upcast<S>,
    S: JavaObject + 'static,
{
    fn as_ref(&self) -> &S {
        // XX: Safety
        unsafe { S::from_raw(self.ptr) }
    }
}

impl<R: JavaObject> Global<R> {
    /// XX: Trying to map onto Into causes impl conflits
    pub fn upcast<S>(self) -> Global<S>
    where
        R: Upcast<S>,
        S: JavaObject + 'static,
    {
        // XX: safety
        let upcast = unsafe { Global::<S>::from_raw(self.ptr) };
        upcast
    }
}

pub trait CloneIn<'jvm> {
    fn clone_in(&self, jvm: &mut Jvm<'jvm>) -> Self;
}

impl<T> CloneIn<'_> for T
where
    T: Clone,
{
    fn clone_in(&self, _jvm: &mut Jvm<'_>) -> Self {
        self.clone()
    }
}

impl<'jvm, T> CloneIn<'jvm> for Local<'jvm, T>
where
    T: JavaObject,
{
    fn clone_in(&self, jvm: &mut Jvm<'jvm>) -> Self {
        jvm.local(self)
    }
}

impl<'jvm, T> CloneIn<'jvm> for Global<T>
where
    T: JavaObject,
{
    fn clone_in(&self, jvm: &mut Jvm<'jvm>) -> Self {
        jvm.global(self)
    }
}

pub trait FromJValue<'jvm> {
    fn from_jvalue(jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self;
}

impl<'jvm, T> FromJValue<'jvm> for Option<Local<'jvm, T>>
where
    T: JavaObject,
{
    fn from_jvalue(jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Object(o) => {
                if o.is_null() {
                    None
                } else {
                    let env = jvm.to_env();
                    Some(unsafe { Local::from_jni(AutoLocal::new(o, &env)) })
                }
            }
            _ => panic!("expected object, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for () {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Void => (),
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for i32 {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Int(i) => i,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for i64 {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Long(i) => i,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for i8 {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Byte(i) => i,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for i16 {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Short(i) => i,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for bool {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Bool(i) => i != 0,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for f32 {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Float(i) => i,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}

impl<'jvm> FromJValue<'jvm> for f64 {
    fn from_jvalue(_jvm: &mut Jvm<'jvm>, value: JValueOwned<'jvm>) -> Self {
        match value {
            jni::objects::JValueGen::Double(i) => i,
            _ => panic!("expected void, found {value:?})"),
        }
    }
}
