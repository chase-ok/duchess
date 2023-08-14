use duchess::{java::lang::{Object, Throwable}, prelude::*, Global, Local, Jvm};

#[test]
fn can_pass_none_as_null_object_params() {
    let obj = Object::new().global().execute().unwrap();

    let null_ref = None::<&Object>;
    assert!(!obj.equals(null_ref).to_rust().execute().unwrap());

    let null_global = None::<Global<Object>>;
    assert!(!obj.equals(&null_global).to_rust().execute().unwrap());

    let null_global_ref = None::<&Global<Object>>;
    assert!(!obj.equals(null_global_ref).to_rust().execute().unwrap());

    let null_local = None::<&Local<Object>>;
    assert!(!obj.equals(null_local).to_rust().execute().unwrap());

    let null_local_ref = None::<&Local<Object>>;
    assert!(!obj.equals(null_local_ref).to_rust().execute().unwrap());
}

#[test]
fn can_pass_some_as_non_null_object_params() {
    let obj = Object::new().global().execute().unwrap();

    let non_null_ref: Option<&Object> = Some(&obj);
    assert!(obj.equals(non_null_ref).to_rust().execute().unwrap());

    let non_null_global = &Some(obj.clone());
    assert!(obj.equals(non_null_global).to_rust().execute().unwrap());

    let non_null_global_ref = Some(&obj);
    assert!(obj.equals(non_null_global_ref).to_rust().execute().unwrap());

    Jvm::with(|jvm| {
        let non_null_local = Some(jvm.local(&*obj));
        assert!(obj.equals(&non_null_local).to_rust().execute_with(jvm).unwrap());

        let local = jvm.local(&*obj);
        let non_null_local_ref = Some(&local);
        assert!(obj.equals(non_null_local_ref).to_rust().execute_with(jvm).unwrap());
        Ok(())
    }).unwrap();
}

#[test]
fn null_method_returns_to_rust_as_none() {
    let throwable_without_message = Throwable::new();
    let null_message = throwable_without_message.get_message().global().execute().unwrap();
    assert!(null_message.is_none());
}