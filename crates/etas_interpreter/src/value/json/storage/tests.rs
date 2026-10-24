use super::*;
use crate::testing::allocation::measure;

#[test]
fn ingress_reuses_buffers_and_sharing_retains_only_live_nodes() {
    let input = vec![Json::Bool(true), Json::Null];
    let pointer = input.as_ptr();
    let (child, array_allocation) = measure(|| JsonArray::from(input));
    assert_eq!(child.as_ptr(), pointer);
    assert_eq!(array_allocation.count, 1, "shared owner, no payload copy");
    let child_weak = Rc::downgrade(&child.0);
    let fields = vec![("child".to_owned(), Json::Array(child.clone()))];
    let pointer = fields.as_ptr();
    let (object, object_allocation) = measure(|| JsonObject::from(fields));
    assert_eq!(object.as_ptr(), pointer);
    assert_eq!(
        object_allocation.count, 1,
        "shared owner, no field buffer copy"
    );
    eprintln!("JSON cold ownership: array={array_allocation:?}, object={object_allocation:?}");
    let object_weak = Rc::downgrade(&object.0);
    let alias = object.clone();
    drop(child);
    drop(object);
    assert!(object_weak.upgrade().is_some());
    assert!(child_weak.upgrade().is_some());
    drop(alias);
    assert!(object_weak.upgrade().is_none());
    assert!(child_weak.upgrade().is_none());
}

#[test]
fn releasing_a_shared_dag_does_not_copy_or_leak_children() {
    let child = JsonArray::from(vec![Json::String("payload".into())]);
    let weak = Rc::downgrade(&child.0);
    let roots = JsonArray::from(
        (0..4000)
            .map(|_| Json::Array(child.clone()))
            .collect::<Vec<_>>(),
    );
    drop(child);
    let (_, allocations) = measure(|| drop(roots));
    assert_eq!(allocations.count, 0);
    assert!(weak.upgrade().is_none());
}
