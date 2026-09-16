use crate::{orchestration::ValueSnapshot, testing::allocation::measure, value::*};

#[test]
fn repeated_lambda_frames_reuse_captured_locals() {
    use crate::{
        control::{CallTarget, Frame},
        orchestration::CallTargetSnapshot,
    };
    use etas_hir::{HirExprId, SymbolId};
    for count in [1000, 2000, 4000] {
        let frame = Frame::from_snapshot(vec![(
            SymbolId(0),
            InterpValue::Array(vec![InterpValue::Bool(true); 128].into()),
        )])
        .unwrap();
        let runtime = InterpValue::Array(
            (0..count)
                .map(|i| {
                    InterpValue::Callable(CallTarget::Lambda {
                        expr: HirExprId(i),
                        captured: frame.clone(),
                    })
                })
                .collect::<Vec<_>>()
                .into(),
        );
        let (captured, cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        eprintln!("lambda frames={count}: {cost:?}");
        assert!(cost.count < 16, "repeated frame capture: {cost:?}");
        let ValueSnapshot::Array(values) = captured else {
            panic!("array")
        };
        let ValueSnapshot::Callable(CallTargetSnapshot::Lambda {
            captured: first, ..
        }) = &values[0]
        else {
            panic!("lambda")
        };
        for (i, value) in values.iter().enumerate() {
            let ValueSnapshot::Callable(CallTargetSnapshot::Lambda { expr, captured }) = value
            else {
                panic!("lambda")
            };
            assert_eq!(expr.0, i as u32);
            assert_eq!(captured.id, first.id);
            assert!(std::rc::Rc::ptr_eq(&captured.locals, &first.locals));
        }
    }
}

#[test]
fn repeated_container_capture_builds_each_shared_table_once() {
    for count in [1000, 2000, 4000] {
        let child = InterpValue::Array(vec![InterpValue::Bool(true); 128].into());
        let runtime = InterpValue::Array(vec![child; count].into());
        let (captured, cost) = measure(|| ValueSnapshot::capture(&runtime).unwrap());
        eprintln!("capture aliases={count}: {cost:?}");
        assert!(cost.count < 16, "rebuilt aliased child tables: {cost:?}");
        let ValueSnapshot::Array(children) = captured else {
            panic!("array")
        };
        let ValueSnapshot::Array(first) = &children[0] else {
            panic!("child")
        };
        for child in &children {
            let ValueSnapshot::Array(child) = child else {
                panic!("child")
            };
            assert_eq!(first.as_ptr(), child.as_ptr());
        }
    }
}

#[test]
fn repeated_frame_locals_share_captured_structure() {
    use crate::control::Frame;
    use etas_hir::SymbolId;
    for count in [1000, 2000, 4000] {
        let child = InterpValue::Tuple(vec![InterpValue::Bool(true); 128].into());
        let frame =
            Frame::from_snapshot((0..count).map(|i| (SymbolId(i), child.clone())).collect())
                .unwrap();
        let (captured, cost) = measure(|| super::frame::capture_frame(&frame).unwrap());
        eprintln!("capture frame aliases={count}: {cost:?}");
        assert!(cost.count < 16, "rebuilt aliased locals: {cost:?}");
        let ValueSnapshot::Tuple(first) = &captured.locals[0].1 else {
            panic!("tuple")
        };
        for (_, value) in captured.locals.iter() {
            let ValueSnapshot::Tuple(value) = value else {
                panic!("tuple")
            };
            assert_eq!(value.as_ptr(), first.as_ptr());
        }
    }
}

#[test]
fn capture_identity_preserves_container_kinds_views_and_nominal_identity() {
    use etas_types::{TrustWrapper, TypeId};
    let array: ArrayValue = vec![InterpValue::i32(7), InterpValue::i32(9)].into();
    let fields: SharedFields = vec![InterpValue::Bool(true)].into();
    let payload: SharedValue = InterpValue::i32(7).into();
    let map: MapValue = vec![(InterpValue::i32(1), InterpValue::i32(2))].into();
    let set: SetValue = vec![InterpValue::i32(1)].into();
    let deque: DequeValue = vec![InterpValue::i32(1)].into();
    let list: ListValue = vec![InterpValue::i32(1), InterpValue::i32(2)].into();
    let record: RecordValue = vec![("x".into(), InterpValue::i32(1))].into();
    let values = vec![
        InterpValue::Array(array.clone()),
        InterpValue::Stack(array.clone()),
        InterpValue::Slice(SliceValue::from_array(array.clone(), 0..1).unwrap()),
        InterpValue::Slice(SliceValue::from_array(array.clone(), 0..2).unwrap()),
        InterpValue::Slice(SliceValue::from_array(array.clone(), 1..2).unwrap()),
        InterpValue::Slice(SliceValue::from_array(array.clone(), 0..0).unwrap()),
        InterpValue::Slice(SliceValue::from_array(array, 1..1).unwrap()),
        InterpValue::Tuple(fields.clone()),
        InterpValue::Variant {
            name: "First".into(),
            fields: fields.clone(),
        },
        InterpValue::Variant {
            name: "Second".into(),
            fields,
        },
        InterpValue::OptionSome(payload.clone()),
        InterpValue::Nominal {
            ty: TypeId(1),
            value: payload.clone(),
        },
        InterpValue::Nominal {
            ty: TypeId(2),
            value: payload.clone(),
        },
        InterpValue::Trust {
            wrapper: TrustWrapper::Trusted,
            value: payload,
        },
        InterpValue::Map(map.clone()),
        InterpValue::OrderedMap(map.clone()),
        InterpValue::PriorityQueue(map),
        InterpValue::Set(set.clone()),
        InterpValue::OrderedSet(set),
        InterpValue::Deque(deque.clone()),
        InterpValue::Queue(deque),
        InterpValue::List(list),
        InterpValue::Record(record),
    ];
    let root = InterpValue::Tuple(
        values
            .iter()
            .chain(values.iter())
            .cloned()
            .collect::<Vec<_>>()
            .into(),
    );
    let captured = ValueSnapshot::capture(&root).unwrap();
    assert!(captured.restore().unwrap() == root);
}

#[test]
fn capture_cache_is_operation_local_and_does_not_merge_equal_allocations() {
    let mut array: ArrayValue = vec![InterpValue::i32(1)].into();
    let other: ArrayValue = vec![InterpValue::i32(1)].into();
    let root = InterpValue::Tuple(
        vec![
            InterpValue::Array(array.clone()),
            InterpValue::Array(other.clone()),
        ]
        .into(),
    );
    let saved = ValueSnapshot::capture(&root).unwrap();
    let ValueSnapshot::Tuple(children) = &saved else {
        panic!("tuple")
    };
    let (ValueSnapshot::Array(first), ValueSnapshot::Array(second)) = (&children[0], &children[1])
    else {
        panic!("arrays")
    };
    assert_ne!(first.as_ptr(), second.as_ptr());
    array.borrow_mut()[0] = InterpValue::i32(9);
    let changed = InterpValue::Array(array.clone());
    let next = ValueSnapshot::capture(&changed).unwrap();
    assert!(next.restore().unwrap() == changed);
    assert!(saved.restore().unwrap() == root);

    // Even empty Vec buffers must be identified by owners, not dangling data pointers.
    let a: ArrayValue = Vec::new().into();
    let b: ArrayValue = Vec::new().into();
    let aliases = (a.clone(), b.clone());
    let ka = super::capture_identity::CaptureIdentity::of(&InterpValue::Array(a)).unwrap();
    let kb = super::capture_identity::CaptureIdentity::of(&InterpValue::Array(b)).unwrap();
    assert!(ka != kb);
    drop(aliases);
}

#[test]
fn cached_payload_never_substitutes_message_envelope_identity() {
    let payload: SharedValue = InterpValue::Array(vec![InterpValue::Bool(true); 128].into()).into();
    let messages = ["first", "second"].map(|id| {
        InterpValue::Message(MessageValue {
            id: id.into(),
            from: None,
            to: None,
            role: MessageRoleValue::User,
            session: None,
            created_at: String::new(),
            payload: payload.clone(),
            provenance: None,
        })
    });
    let root = InterpValue::Tuple(messages.to_vec().into());
    assert!(ValueSnapshot::capture(&root).unwrap().restore().unwrap() == root);
}

#[test]
fn deep_shared_capture_and_failed_capture_release_cached_graphs() {
    const WORKER: &str = "ETAS_CAPTURE_SHARED_GRAPH_WORKER";
    if std::env::var_os(WORKER).is_none() {
        let thread = std::thread::current();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", thread.name().unwrap(), "--nocapture"])
            .env(WORKER, "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let runtime = (0..30_000).fold(InterpValue::Bool(true), |value, _| {
        InterpValue::Tuple(vec![value.clone(), value].into())
    });
    let (_, cost) = measure(|| {
        let captured = ValueSnapshot::capture(&runtime).unwrap();
        let mut cursor = &captured;
        for _ in 0..30_000 {
            let ValueSnapshot::Tuple(children) = cursor else {
                panic!("tuple")
            };
            if let (ValueSnapshot::Tuple(a), ValueSnapshot::Tuple(b)) = (&children[0], &children[1])
            {
                assert_eq!(a.as_ptr(), b.as_ptr());
            }
            cursor = &children[0];
        }
        assert!(matches!(cursor, ValueSnapshot::Bool(true)));
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "cached snapshot leaked: {cost:?}"
    );

    let handle = InterpValue::HostHandle(HostHandleValue::tcp_stream(
        etas_types::TypeId(1),
        etas_host::TcpStreamRef::issued(
            etas_host::StreamHandleRef::issued("live", 0),
            etas_host::ByteStreamOrigin::Tcp {
                host: "example.test".into(),
                port: 443,
            },
        ),
    ));
    let root = InterpValue::Tuple(vec![runtime.clone(), runtime, handle].into());
    let (_, cost) = measure(|| {
        let error = ValueSnapshot::capture(&root).unwrap_err();
        assert!(error.contains("live tcp_stream host handles"), "{error}");
    });
    assert_eq!(
        cost.bytes, cost.released_bytes,
        "failed capture leaked: {cost:?}"
    );
}
