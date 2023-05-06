use std::cell::Cell;

use crate::{
    raw::{EnvPtr, JvmPtr},
    Error, GlobalResult,
};

enum State {
    InUse,
    Attached(EnvPtr<'static>),
    Detached,
}

// XX: if we're being invoked by Java, can we clear this state for recursion?
thread_local! {
    static STATE: Cell<State> = Cell::new(State::Detached);
}

fn attached_or(
    jvm: JvmPtr,
    f: impl FnOnce() -> GlobalResult<AttachGuard>,
) -> GlobalResult<AttachGuard> {
    STATE.with(|state| match state.replace(State::InUse) {
        State::Attached(env) => Ok(AttachGuard {
            jvm,
            env,
            permanent: true,
        }),
        State::InUse => Err(Error::NestedUsage),
        State::Detached => {
            let result = f();
            if result.is_err() {
                state.set(State::Detached);
            }
            result
        }
    })
}

pub(crate) fn attach_permanently(jvm: JvmPtr) -> GlobalResult<AttachGuard> {
    attached_or(jvm, || {
        Ok(AttachGuard {
            jvm,
            // no-op if already attached outside of duchess
            env: unsafe { jvm.attach_thread()? },
            permanent: true,
        })
    })
}

pub(crate) unsafe fn attach<'jvm>(jvm: JvmPtr) -> GlobalResult<AttachGuard> {
    attached_or(jvm, || {
        Ok(AttachGuard {
            jvm,
            // no-op if already attached outside of duchess
            env: unsafe { jvm.attach_thread()? },
            permanent: false,
        })
    })
}

pub struct AttachGuard {
    jvm: JvmPtr,
    env: EnvPtr<'static>,
    permanent: bool,
}

impl Drop for AttachGuard {
    fn drop(&mut self) {
        if self.permanent {
            STATE.with(|state| {
                let state = state.replace(State::Attached(self.env));
                debug_assert!(matches!(state, State::InUse))
            });
        } else {
            match unsafe { self.jvm.detach_thread() } {
                Ok(()) => STATE.with(|state| state.set(State::Detached)),
                Err(e) => {
                    // XX
                    println!("couldn't detach: {}", e);
                }
            }
        }
    }
}

impl AttachGuard {
    pub fn env(&mut self) -> EnvPtr<'_> {
        self.env
    }
}
