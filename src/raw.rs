use std::{
    ffi,
    marker::PhantomData,
    ptr::{self, NonNull},
};

use jni_sys::jvalue;

use crate::{jvm::JavaObjectExt, Error, GlobalResult, JavaObject, Jvm, Local};

const VERSION: jni_sys::jint = jni_sys::JNI_VERSION_1_8;

pub fn jvm() -> GlobalResult<Option<JvmPtr>> {
    let libjvm = crate::libjvm::libjvm_or_load()?;

    let mut jvms = [std::ptr::null_mut::<jni_sys::JavaVM>()];
    let mut num_jvms: jni_sys::jsize = 0;

    let code = unsafe {
        (libjvm.JNI_GetCreatedJavaVMs)(
            jvms.as_mut_ptr(),
            jvms.len().try_into().unwrap(),
            &mut num_jvms as *mut _,
        )
    };
    if code != jni_sys::JNI_OK {
        return Err(Error::JvmInternal(format!(
            "GetCreatedJavaVMs failed with code `{code}`"
        )));
    }

    match num_jvms {
        0 => Ok(None),
        1 => JvmPtr::new(jvms[0])
            .ok_or_else(|| Error::JvmInternal("GetCreatedJavaVMs returned null pointer".into()))
            .map(Some),
        _ => Err(Error::JvmInternal(format!(
            "GetCreatedJavaVMs returned more JVMs than expected: `{num_jvms}`"
        ))),
    }
}

pub fn create_jvm<'a>(options: impl IntoIterator<Item = &'a str>) -> GlobalResult<JvmPtr> {
    let libjvm = crate::libjvm::libjvm_or_load()?;

    let options = options
        .into_iter()
        .map(|opt| ffi::CString::new(opt).unwrap())
        .collect::<Vec<_>>();

    let mut option_ptrs = options
        .iter()
        .map(|opt| jni_sys::JavaVMOption {
            optionString: opt.as_ptr().cast_mut(),
            extraInfo: std::ptr::null_mut(),
        })
        .collect::<Vec<_>>();

    let mut args = jni_sys::JavaVMInitArgs {
        version: VERSION,
        nOptions: options.len().try_into().unwrap(),
        options: option_ptrs.as_mut_ptr(),
        ignoreUnrecognized: jni_sys::JNI_FALSE,
    };

    let mut jvm = std::ptr::null_mut::<jni_sys::JavaVM>();
    let mut env = std::ptr::null_mut::<ffi::c_void>();
    let code = unsafe {
        (libjvm.JNI_CreateJavaVM)(
            &mut jvm as *mut _,
            &mut env as *mut _,
            &mut args as *mut _ as *mut ffi::c_void,
        )
    };

    if code == jni_sys::JNI_OK {
        JvmPtr::new(jvm)
            .ok_or_else(|| Error::JvmInternal("CreateJavaVM returned null JVM pointer".into()))
    } else {
        Err(Error::JvmInternal(format!(
            "CreateJavaVM failed with code `{code}`"
        )))
    }
}

#[derive(Clone, Copy)]
pub struct JvmPtr(NonNull<jni_sys::JavaVM>);

impl JvmPtr {
    pub fn new(ptr: *mut jni_sys::JavaVM) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }

    pub unsafe fn env<'jvm>(self) -> GlobalResult<Option<EnvPtr<'jvm>>> {
        let mut env_ptr = std::ptr::null_mut::<ffi::c_void>();
        match fn_table_call(
            self.0,
            |jvm| jvm.GetEnv,
            |jvm, f| f(jvm, &mut env_ptr as *mut _, VERSION),
        ) {
            jni_sys::JNI_OK => Ok(Some(EnvPtr::new(env_ptr.cast()).unwrap())),
            jni_sys::JNI_EDETACHED => Ok(None),
            code => Err(Error::JvmInternal(format!(
                "GetEnv failed with code `{code}`"
            ))),
        }
    }

    pub unsafe fn attach_thread<'jvm>(self) -> GlobalResult<EnvPtr<'jvm>> {
        let mut env_ptr = std::ptr::null_mut::<ffi::c_void>();
        match fn_table_call(
            self.0,
            |jvm| jvm.AttachCurrentThread,
            |jvm, f| {
                f(
                    jvm,
                    &mut env_ptr as *mut _,
                    std::ptr::null_mut(), /* args */
                )
            },
        ) {
            jni_sys::JNI_OK => Ok(EnvPtr::new(env_ptr.cast()).unwrap()),
            code => Err(Error::JvmInternal(format!(
                "AttachCurrentThread failed with code `{code}`"
            ))),
        }
    }

    pub unsafe fn detach_thread(self) -> GlobalResult<()> {
        match fn_table_call(self.0, |jvm| jvm.DetachCurrentThread, |jvm, f| f(jvm)) {
            jni_sys::JNI_OK => Ok(()),
            code => Err(Error::JvmInternal(format!(
                "DetachCurrentThread failed with code `{code}`"
            ))),
        }
    }
}

// XX
unsafe fn fn_table_call<T, F, R>(
    table_ptr: NonNull<*const T>,
    fn_field: impl FnOnce(&T) -> Option<F>,
    call: impl FnOnce(*mut *const T, F) -> R,
) -> R {
    let fn_field = fn_field(&**table_ptr.as_ptr());
    let fn_field = fn_field.unwrap_unchecked();
    call(table_ptr.as_ptr(), fn_field)
}

// XX safety
unsafe impl Send for JvmPtr {}
unsafe impl Sync for JvmPtr {}

// XX: jvm lifetime
#[derive(Clone, Copy)]
pub struct EnvPtr<'jvm> {
    ptr: NonNull<jni_sys::JNIEnv>,
    _marker: PhantomData<&'jvm ()>,
}

impl<'jvm> EnvPtr<'jvm> {
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
        fn_table_call(self.ptr, fn_field, call)
    }
}

// XX EnvPtr isn't send/sync

// XX: hiding behind this trait to avoid exposing through Jvm struct
#[doc(hidden)]
pub trait HasEnvPtr<'jvm> {
    fn env(&self) -> EnvPtr<'jvm>;
}

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

/// XXX
pub trait IntoJniValue {
    fn into_jni_value(self) -> jvalue;
}

impl<T: JavaObject> IntoJniValue for &T {
    fn into_jni_value(self) -> jvalue {
        jvalue {
            l: self.as_raw().as_ptr(),
        }
    }
}

impl<T: JavaObject> IntoJniValue for Option<&T> {
    fn into_jni_value(self) -> jvalue {
        self.map(|v| v.into_jni_value())
            .unwrap_or(jvalue { l: ptr::null_mut() })
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
        ObjectPtr::new(value).map(|obj| unsafe { Local::from_raw(jvm.env(), obj) })
    }
}

// () is Java `void`
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
