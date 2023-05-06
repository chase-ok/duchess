use crate::{
    cast::{AsUpcast, TryDowncast, Upcast},
    catch::{CatchNone, Catching},
    find::find_class,
    inspect::{ArgOp, Inspect},
    java::lang::{Class, ClassExt, Throwable},
    not_null::NotNull,
    raw::{self, EnvPtr, HasEnvPtr, JvmPtr, ObjectPtr},
    thread, Global, IntoLocal, Local,
};

use std::{env, ffi::CStr, path::Path, ptr::NonNull};

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

static GLOBAL_JVM: Lazy<JvmPtr> = Lazy::new(|| {
    if let Some(jvm) = raw::jvm().unwrap() {
        return jvm;
    }

    let mut options = vec!["-Xcheck:jni".to_owned()];
    if let Ok(classpath) = env::var("CLASSPATH") {
        options.push(format!("-Djava.class.path={classpath}"));
    }

    raw::create_jvm(options.iter().map(|s| s.as_str())).unwrap()
});

pub struct Jvm<'jvm> {
    jvm: JvmPtr,
    env: EnvPtr<'jvm>,
}

impl Jvm<'_> {
    pub fn attach_thread_permanently() -> crate::GlobalResult<()> {
        thread::attach_permanently(*GLOBAL_JVM)?;
        Ok(())
    }
}

impl<'jvm> Jvm<'jvm> {
    pub fn load_libjvm_at(path: impl AsRef<Path>) -> crate::GlobalResult<()> {
        crate::libjvm::libjvm_or_load_at(path.as_ref())?;
        Ok(())
    }

    pub fn with<R>(
        op: impl for<'a> FnOnce(&mut Jvm<'a>) -> crate::Result<'a, R>,
    ) -> crate::GlobalResult<R> {
        let jvm = *GLOBAL_JVM;
        let mut guard = unsafe { thread::attach(jvm)? };

        let mut jvm = Jvm {
            jvm,
            env: guard.env(),
        };

        op(&mut jvm).map_err(|e| e.into_global(&mut jvm))
    }

    pub fn local<R>(&mut self, r: &R) -> Local<'jvm, R>
    where
        R: JavaObject,
    {
        Local::new(self.env, r)
    }

    pub fn global<R>(&mut self, r: &R) -> Global<R>
    where
        R: JavaObject,
    {
        Global::new(self.jvm, self.env, r)
    }
}

impl<'jvm> HasEnvPtr<'jvm> for Jvm<'jvm> {
    fn env(&self) -> EnvPtr<'jvm> {
        self.env
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

    unsafe fn from_raw<'a>(ptr: ObjectPtr) -> &'a Self;
    fn as_raw(&self) -> ObjectPtr;
}
impl<T: JavaObject> JavaObjectExt for T {
    unsafe fn from_raw<'a>(ptr: ObjectPtr) -> &'a Self {
        // XX: safety
        unsafe { ptr.as_ref() }
    }

    fn as_raw(&self) -> ObjectPtr {
        // XX: safety
        unsafe { NonNull::new_unchecked((self as *const Self).cast_mut()).cast() }.into()
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
                    // XX: Safety
                    const CLASS_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked($array_class) };
                    static CLASS: OnceCell<Global<crate::java::lang::Class>> = OnceCell::new();

                    let global = CLASS.get_or_try_init::<_, crate::Error<Local<Throwable>>>(|| {
                        let class = find_class(jvm, CLASS_NAME)?;
                        Ok(jvm.global(&class))
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
