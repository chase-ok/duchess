use duchess::{java, prelude::*, Global};

#[test]
fn to_java_and_back() {
    for example in ["", "abc\tdef", "hello from 🦀!"] {
        // XX: remove null?
        let java: Global<java::lang::String> = example.to_java().assert_not_null().global().execute().unwrap();
        dbg!(example);
        let and_back = (&*java).to_rust().execute().unwrap();
        assert_eq!(example, and_back);
    }
}