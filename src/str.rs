use std::ffi::CString;

use crate::{
    error::check_exception,
    java::lang::String as JavaString,
    jvm::JavaObjectExt,
    ops::{IntoJava, IntoRust},
    raw::ObjectPtr,
    Error, Jvm, JvmOp, Local,
};

impl IntoJava<JavaString> for &str {
    type Output<'jvm> = Local<'jvm, JavaString>;

    fn into_java<'jvm>(self, jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, Local<'jvm, JavaString>> {
        let encoded = cesu8::to_java_cesu8(self);
        // XX: Safety: cesu8 encodes interior nul bytes as 0xC080
        let c_string = unsafe { CString::from_vec_unchecked(encoded.into_owned()) };

        let jni = jvm.as_raw();
        let string = unsafe { jni.invoke(|jni| jni.NewStringUTF, |jni, f| f(jni, c_string.as_ptr())) };
        if let Some(string) = ObjectPtr::new(string) {
            Ok(unsafe { Local::from_raw(jni, string) })
        } else {
            check_exception(jvm)?; // likely threw an OutOfMemoryError
            Err(Error::JvmInternal("JVM failed to create new String".into()))
        }
    }
}

impl IntoJava<JavaString> for String {
    type Output<'jvm> = Local<'jvm, JavaString>;

    fn into_java<'jvm>(self, jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, Local<'jvm, JavaString>> {
        <&str as IntoJava<JavaString>>::into_java(&self, jvm)
    }
}

impl<J> IntoRust<String> for J
where
    for<'jvm> J: JvmOp<Input<'jvm> = ()>,
    for<'jvm> J::Output<'jvm>: AsRef<JavaString>,
{
    fn into_rust<'jvm>(self, jvm: &mut Jvm<'jvm>) -> crate::Result<'jvm, String> {
        let object = self.execute_with(jvm, ())?;
        let str_raw = object.as_ref().as_raw();

        let jni = jvm.as_raw();

        let cesu8_len = unsafe { jni.invoke(|jni| jni.GetStringUTFLength, |jni, f| f(jni, str_raw.as_ptr())) };
        assert!(cesu8_len >= 0);
        let utf16_len = unsafe { jni.invoke(|jni| jni.GetStringLength, |jni, f| f(jni, str_raw.as_ptr())) };
        assert!(utf16_len >= 0);

        let mut cesu_bytes = Vec::<u8>::with_capacity(cesu8_len as usize);
        // XX: safety
        unsafe {
            jni.invoke(|jni| jni.GetStringUTFRegion, |jni, f| f(
                jni,
                str_raw.as_ptr(),
                0,
                utf16_len,
                cesu_bytes.as_mut_ptr().cast::<i8>(),
            ));
            cesu_bytes.set_len(cesu8_len as usize);
        };
        check_exception(jvm)?;

        let decoded = cesu8::from_java_cesu8(cesu_bytes.as_slice())
            .map_err(|e| {
                Error::JvmInternal(format!(
                    "Java String contained invalid modified UTF-8: {}",
                    e
                ))
            })?
            .into_owned();
        Ok(decoded)
    }
}
