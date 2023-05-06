use std::{marker::PhantomData, ops::Deref, ptr::NonNull};

use crate::jvm::JavaObjectExt;
use crate::{
    cast::Upcast,
    jvm::CloneIn,
    plumbing::ObjectPtr,
    raw::{EnvPtr, JvmPtr},
    JavaObject, Jvm,
};

/// An owned local reference to a non-null Java object of type `T`. The reference will be freed when
/// dropped.
pub struct Local<'jvm, T: JavaObject> {
    env: EnvPtr<'jvm>,
    obj: ObjectPtr,
    _marker: PhantomData<T>,
}

impl<'jvm, T: JavaObject> Local<'jvm, T> {
    // XX
    pub unsafe fn from_raw(env: EnvPtr<'jvm>, obj: ObjectPtr) -> Self {
        Self {
            obj,
            env,
            _marker: PhantomData,
        }
    }

    pub fn new(env: EnvPtr<'jvm>, obj: &T) -> Self {
        // XX: safety
        unsafe {
            let new_ref = env.invoke(
                |jni| jni.NewLocalRef,
                |jni, f| f(jni, obj.as_raw().as_ptr()),
            );
            Self::from_raw(env, NonNull::new(new_ref).unwrap().into())
        }
    }
}

impl<T: JavaObject> Drop for Local<'_, T> {
    fn drop(&mut self) {
        // XX safety
        unsafe {
            self.env
                .invoke(|jni| jni.DeleteLocalRef, |jni, f| f(jni, self.obj.as_ptr()));
        }
    }
}

impl<T: JavaObject> Deref for Local<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // XX: safety
        unsafe { T::from_raw(self.obj) }
    }
}

/// An owned global reference to a non-null Java object of type `T`. The reference will be freed
/// when dropped.
/// XX: not wrapped in Arc like jni
pub struct Global<T: JavaObject> {
    jvm: JvmPtr,
    obj: ObjectPtr,
    _marker: PhantomData<T>,
}

impl<T: JavaObject> Global<T> {
    // XX
    pub(crate) unsafe fn from_raw(jvm: JvmPtr, obj: ObjectPtr) -> Self {
        Self {
            jvm,
            obj,
            _marker: PhantomData,
        }
    }

    pub(crate) fn new(jvm: JvmPtr, env: EnvPtr<'_>, obj: &T) -> Self {
        unsafe {
            let new_ref = env.invoke(|e| e.NewGlobalRef, |e, f| f(e, obj.as_raw().as_ptr()));
            Self::from_raw(jvm, NonNull::new(new_ref).unwrap().into())
        }
    }
}

impl<T: JavaObject> Drop for Global<T> {
    fn drop(&mut self) {
        let delete = |env: EnvPtr<'_>| unsafe {
            env.invoke(
                |jni| jni.DeleteGlobalRef,
                |jni, f| f(jni, self.obj.as_ptr()),
            )
        };
        // XX: safety
        match unsafe { self.jvm.env() } {
            Ok(Some(env)) => delete(env),
            Ok(None) => {
                match unsafe { self.jvm.attach_thread() } {
                    Ok(env) => delete(env), // XX: detach guard
                    Err(_e) => {}           // trace debug
                }
            }
            Err(_e) => {
                // XX: trace debug message on error
            }
        }
    }
}

impl<T: JavaObject> Deref for Global<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // XX: safety
        unsafe { T::from_raw(self.obj) }
    }
}

impl<'a, R, S> AsRef<S> for Local<'a, R>
where
    R: Upcast<S>,
    S: JavaObject + 'a,
{
    fn as_ref(&self) -> &S {
        // XX: Safety
        unsafe { S::from_raw(self.obj) }
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
        let upcast = unsafe { Local::<S>::from_raw(self.env, self.obj) };
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
        unsafe { S::from_raw(self.obj) }
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
        let upcast = unsafe { Global::<S>::from_raw(self.jvm, self.obj) };
        upcast
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
