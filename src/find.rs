use std::ffi::CStr;

use crate::{java, plumbing::check_exception, raw::{ObjectPtr, MethodPtr}, Jvm, Local, Result, jvm::JavaObjectExt};

pub fn find_class<'jvm>(
    jvm: &mut Jvm<'jvm>,
    jni_name: &CStr,
) -> Result<'jvm, Local<'jvm, java::lang::Class>> {
    let jni = jvm.as_raw();
    let class = unsafe { jni.invoke(|jni| jni.FindClass, |jni, f| f(jni, jni_name.as_ptr())) };
    if let Some(class) = ObjectPtr::new(class) {
        Ok(unsafe { Local::from_raw(jni, class) })
    } else {
        check_exception(jvm)?; 
        // Class not existing should've triggered NoClassDefFoundError so something strange is now happening
        Err(crate::Error::JvmInternal(format!(
            "failed to find class `{}`",
            jni_name.to_string_lossy()
        )))
    }
}

pub fn find_method<'jvm>(
    jvm: &mut Jvm<'jvm>,
    class: impl AsRef<java::lang::Class>,
    jni_name: &CStr,
    jni_descriptor: &CStr,
) -> Result<'jvm, MethodPtr> {
    let class = class.as_ref().as_raw();

    let jni = jvm.as_raw();
    let method = unsafe { jni.invoke(|jni| jni.GetMethodID, |jni, f| f(jni, class.as_ptr(), jni_name.as_ptr(), jni_descriptor.as_ptr())) };
    if let Some(method) = MethodPtr::new(method) {
        Ok(method)
    } else {
        check_exception(jvm)?; 
        // Method not existing should've triggered NoSuchMethodError so something strange is now happening
        Err(crate::Error::JvmInternal(format!(
            "failed to find method `{}` with signature `{}`",
            jni_name.to_string_lossy(), jni_descriptor.to_string_lossy(),
        )))
    }
}

pub fn find_constructor<'jvm>(
    jvm: &mut Jvm<'jvm>,
    class: impl AsRef<java::lang::Class>,
    jni_descriptor: &CStr,
) -> Result<'jvm, MethodPtr> {
    const METHOD_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"<init>\0") };
    find_method(jvm, class, METHOD_NAME, jni_descriptor)
}