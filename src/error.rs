use std::{
    fmt::{Debug, Display},
    result,
};

use thiserror::Error;

use crate::{
    java::lang::Throwable,
    raw::{HasEnvPtr, ObjectPtr},
    Global, Jvm, Local,
};

/// Result returned by most Java operations that may contain a local reference
/// to a thrown exception.
pub type Result<'jvm, T> = result::Result<T, Error<Local<'jvm, Throwable>>>;

/// Result returned by [`crate::Jvm::with()`] that will store any uncaught
/// exception as a global reference.
pub type GlobalResult<T> = result::Result<T, Error<Global<Throwable>>>;

#[derive(Error)]
pub enum Error<T> {
    /// A reference to an uncaught Java exception
    #[error("Java invocation threw")]
    Thrown(T),

    #[error(
        "slice was too long (`{0}`) to convert to a Java array, which are limited to `i32::MAX`"
    )]
    SliceTooLong(usize),

    #[error("attempted to deref a null Java object pointer")]
    NullDeref,

    #[error("attempted to nest `Jvm::with` calls")]
    NestedUsage,

    #[cfg(feature = "dynlib")]
    #[error(transparent)]
    UnableToLoadLibjvm(#[from] Box<dyn std::error::Error + Send + 'static>),

    /// XX: name?
    #[error("{0}")]
    JvmInternal(String),
}

impl<T> Debug for Error<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

impl<'jvm> Error<Local<'jvm, Throwable>> {
    pub fn into_global(self, jvm: &mut Jvm<'jvm>) -> Error<Global<Throwable>> {
        match self {
            Error::Thrown(t) => Error::Thrown(jvm.global(&t)),
            Error::SliceTooLong(s) => Error::SliceTooLong(s),
            Error::NullDeref => Error::NullDeref,
            Error::NestedUsage => Error::NestedUsage,
            #[cfg(feature = "dynlib")]
            Error::UnableToLoadLibjvm(e) => Error::UnableToLoadLibjvm(e),
            Error::JvmInternal(m) => Error::JvmInternal(m),
        }
    }
}

/// XX
pub fn check_exception<'jvm>(jvm: &mut Jvm<'jvm>) -> Result<'jvm, ()> {
    let env = jvm.env();
    let thrown = unsafe { env.invoke(|env| env.ExceptionOccurred, |env, f| f(env)) };
    if let Some(thrown) = ObjectPtr::new(thrown) {
        unsafe { env.invoke(|env| env.ExceptionClear, |env, f| f(env)) };
        Err(Error::Thrown(unsafe { Local::from_raw(env, thrown) }))
    } else {
        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::{
//         java::{self, lang::ThrowableExt, util::ArrayListExt},
//         prelude::*,
//     };

//     #[test]
//     fn with_jni_env_materializes_thrown_exceptions() {
//         Jvm::with(|jvm| {
//             let env = jvm.to_env();
//             // set exception state
//             env.throw_new("java/lang/Throwable", "some thrown exception")
//                 .unwrap();
//             let err = with_jni_env(env, |_env| Err::<(), _>(jni::errors::Error::JavaException))
//                 .unwrap_err();
//             let Error::Thrown(t) = err else { panic!("expected materialized exception"); };
//             assert_eq!(
//                 "some thrown exception",
//                 t.get_message().assert_not_null().into_rust(jvm).unwrap()
//             );
//             Ok(())
//         })
//         .unwrap();
//     }

//     #[test]
//     fn with_jni_env_maps_everything_else_to_jni() {
//         Jvm::with(|jvm| {
//             let err = with_jni_env(jvm.to_env(), |_env| {
//                 Err::<(), _>(jni::errors::Error::TryLock)
//             })
//             .unwrap_err();
//             assert!(matches!(err, Error::Jni(_)));
//             Ok(())
//         })
//         .unwrap();
//     }

//     // XX this should likely move to an integration test suite
//     #[test]
//     fn exceptions_from_duchess_generated_types_are_materialized_without_a_catch() {
//         Jvm::with(|jvm| {
//             let list = java::util::ArrayList::<java::lang::Object>::new()
//                 .execute(jvm)
//                 .unwrap();
//             // -2 is an illegal from index, throws
//             let err = list.sub_list(-2, 0).execute(jvm).err().unwrap();
//             assert!(matches!(err, Error::Thrown(_)));
//             Ok(())
//         })
//         .unwrap();
//     }
// }
