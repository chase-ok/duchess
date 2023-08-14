use duchess::{java, prelude::*, Global};

#[test]
fn catch_exception_thrown_by_constructor() {
    // new String((byte[]) null) will throw a NullPointerException
    let result = java::lang::String::new(&None::<Global<java::Array<i8>>>)
        .global()
        .execute();
    assert!(matches!(result, Err(duchess::Error::Thrown(_))));
}

#[test]
fn catch_exception_thrown_by_method() {
    let java_string = "abc".to_java().assert_not_null().global().execute().unwrap();
    // invalid indexes
    let result = java_string.substring(1, -1).global().execute();
    assert!(matches!(result, Err(duchess::Error::Thrown(_))));
}