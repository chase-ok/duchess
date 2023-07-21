
use duchess::{prelude::*, java, Global};

#[test]
fn catch_exception_thrown_by_constructor() {
    let result = java::lang::String::new(&None::<Global<java::Array<i8>>>).global().execute();
    assert!(matches!(result, Err(duchess::Error::Thrown(_))));
}