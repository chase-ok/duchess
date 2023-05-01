use std::{marker::PhantomData, ptr::{NonNull, self}};

use jni_sys::jvalue;

use crate::{JavaObject, jvm::JavaObjectExt, Jvm, Local};

#[derive(Clone, Copy)]
pub struct ObjectPtr(NonNull<jni_sys::_jobject>);

impl ObjectPtr {
    pub fn new(ptr: jni_sys::jobject) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    pub fn as_ptr(self) -> jni_sys::jobject {
        self.0.as_ptr()
    }

    pub unsafe fn as_ref<'a, T: JavaObject>(self) -> &'a T {
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
        unsafe { self.0.cast().as_ref() }
    }
}

impl From<NonNull<jni_sys::_jobject>> for ObjectPtr {
    fn from(ptr: NonNull<jni_sys::_jobject>) -> Self {
        Self(ptr)
    }
}

// XX safety
unsafe impl Send for ObjectPtr {}
unsafe impl Sync for ObjectPtr {}

// XX: jvm lifetime
#[derive(Clone, Copy)]
pub struct JniPtr<'jvm> {
    ptr: NonNull<jni_sys::JNIEnv>,
    _marker: PhantomData<&'jvm ()>,
}

impl<'jvm> JniPtr<'jvm> {
    pub unsafe fn new(ptr: *mut jni_sys::JNIEnv) -> Option<Self> {
        let ptr = NonNull::new(ptr)?;
        Some(Self {
            ptr,
            _marker: PhantomData,
        })
    }

    pub unsafe fn invoke<F, T>(
        self,
        fn_field: impl FnOnce(&jni_sys::JNINativeInterface_) -> Option<F>,
        call: impl FnOnce(*mut jni_sys::JNIEnv, F) -> T,
    ) -> T {
        let fn_field = fn_field(&**self.ptr.as_ptr());
        let fn_field = unsafe { fn_field.unwrap_unchecked() }; // XX: JNI fn pointer shouldn't null
        call(self.ptr.as_ptr(), fn_field)
    }
}

// XX JniPtr isn't send/sync

#[derive(Clone, Copy)]
pub struct MethodPtr(NonNull<jni_sys::_jmethodID>);

impl MethodPtr {
    pub fn new(ptr: jni_sys::jmethodID) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    pub fn as_ptr(self) -> jni_sys::jmethodID {
        self.0.as_ptr()
    }
}

// XX safety
unsafe impl Send for MethodPtr {}
unsafe impl Sync for MethodPtr {}

pub trait IntoJniValue {
    fn into_jni_value(self) -> jvalue;
}

impl<T: JavaObject> IntoJniValue for &T {
    fn into_jni_value(self) -> jvalue {
        jvalue {
            l: self.as_raw().as_ptr()
        }
    }
}

impl<T: JavaObject> IntoJniValue for Option<&T> {
    fn into_jni_value(self) -> jvalue {
        self.map(|v| v.into_jni_value()).unwrap_or(jvalue {
            l: ptr::null_mut(),
        })
    }
}

pub trait FromJniValue<'jvm> {
    type JniValue;
    unsafe fn from_jni_value(jvm: &mut Jvm<'jvm>, value: Self::JniValue) -> Self;
}

impl<'jvm, T: JavaObject> FromJniValue<'jvm> for Option<Local<'jvm, T>> {
    type JniValue = jni_sys::jobject;

    unsafe fn from_jni_value(jvm: &mut Jvm<'jvm>, value: Self::JniValue) -> Self {
        // XX safety
        ObjectPtr::new(value).map(|obj| unsafe { Local::from_raw(jvm.as_raw(), obj)})
    }
}

impl<'jvm> FromJniValue<'jvm> for () {
    type JniValue = ();

    unsafe fn from_jni_value(_jvm: &mut Jvm<'jvm>, _value: Self::JniValue) -> Self {
        ()
    }
}

macro_rules! scalar_jni_value {
    ($($rust:ty: $field:ident $java:ident,)*) => {
        $(
            impl IntoJniValue for $rust {
                fn into_jni_value(self) -> jvalue {
                    jvalue {
                        $field: self as jni_sys::$java,
                    }
                }
            }

            impl<'jvm> FromJniValue<'jvm> for $rust {
                type JniValue = jni_sys::$java;

                unsafe fn from_jni_value(_jvm: &mut Jvm<'jvm>, value: Self::JniValue) -> Self {
                    value
                }
            }
        )*
    };
}

scalar_jni_value! {
    // jboolean is u8, need to explicitly map to Rust bool
    // bool: z jboolean,
    i8: b jbyte,
    i16: s jshort,
    u16: c jchar,
    i32: i jint,
    i64: j jlong,
    f32: f jfloat,
    f64: d jdouble,
}

impl IntoJniValue for bool {
    fn into_jni_value(self) -> jvalue {
        jvalue {
            z: self as jni_sys::jboolean,
        }
    }
}

impl<'jvm> FromJniValue<'jvm> for bool {
    type JniValue = jni_sys::jboolean;

    unsafe fn from_jni_value(_jvm: &mut Jvm<'jvm>, value: Self::JniValue) -> Self {
        value == jni_sys::JNI_TRUE
    }
}