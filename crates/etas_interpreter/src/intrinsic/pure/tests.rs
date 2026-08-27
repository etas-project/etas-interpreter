use etas_builtin::{BuiltinError, BuiltinRangeBounds, BuiltinValue};
use etas_std::{StdIntrinsicId, intrinsic};
use etas_types::{
    EnumTypeRef, FieldType, NominalTypeRef, PrimitiveType, RecordType, Type, TypeInterner,
};

use crate::intrinsic::dispatch::CheckedPureIntrinsicCall;
use crate::value::{InterpValue, ListValue, RangeBounds, RangeValue, RecordValue, SliceValue};

use super::abi::input::into_builtin;
use super::abi::output::from_builtin;
use super::{AdapterError, PureAbiProjector, execute_pure_intrinsic};

#[test]
fn adapter_preserves_slice_values() {
    let value = InterpValue::Slice(SliceValue::new(vec![
        InterpValue::i32(1),
        InterpValue::i32(2),
    ]));

    assert_eq!(
        into_builtin(value).expect("slice should convert to builtin"),
        BuiltinValue::Slice(vec![BuiltinValue::I32(1), BuiltinValue::I32(2)])
    );

    assert_eq!(
        from_builtin(BuiltinValue::Slice(vec![
            BuiltinValue::I32(3),
            BuiltinValue::I32(4),
        ]))
        .expect("slice should convert from builtin"),
        InterpValue::Slice(SliceValue::new(vec![
            InterpValue::i32(3),
            InterpValue::i32(4)
        ]))
    );
}

#[test]
fn result_and_option_constructors_preserve_nominal_payloads() {
    let mut interner = TypeInterner::new();
    let string = interner.primitive(PrimitiveType::String);
    let payload_representation = interner.intern(Type::Record(RecordType {
        fields: vec![FieldType {
            name: "name".to_owned(),
            ty: string,
        }],
    }));
    let payload_type = interner.intern(Type::Nominal(NominalTypeRef {
        name: "Payload".to_owned(),
        params: Vec::new(),
        representation: Some(payload_representation),
    }));
    let result_type = interner.intern(Type::Result {
        ok: payload_type,
        err: payload_type,
    });
    let option_type = interner.intern(Type::Option(payload_type));
    let store = interner.into_store();
    let projector = PureAbiProjector::build(&store).expect("checked ABI shapes should build");
    let payload = InterpValue::Nominal {
        ty: payload_type,
        value: Box::new(InterpValue::Record(RecordValue::new(vec![(
            "name".to_owned(),
            InterpValue::String("value".to_owned()),
        )]))),
    };

    let ok = execute_pure_intrinsic(
        &CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(intrinsic::pure::RESULT_OK),
            parameter_types: vec![payload_type],
            result_type,
        },
        vec![payload.clone()],
        &projector,
    )
    .expect("Ok constructor");
    let err = execute_pure_intrinsic(
        &CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(intrinsic::pure::RESULT_ERR),
            parameter_types: vec![payload_type],
            result_type,
        },
        vec![payload.clone()],
        &projector,
    )
    .expect("Err constructor");
    let some = execute_pure_intrinsic(
        &CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(intrinsic::pure::OPTION_SOME),
            parameter_types: vec![payload_type],
            result_type: option_type,
        },
        vec![payload.clone()],
        &projector,
    )
    .expect("Some constructor");

    assert_eq!(
        ok,
        InterpValue::Variant {
            name: "Ok".to_owned(),
            fields: vec![payload.clone()],
        }
    );
    assert_eq!(
        err,
        InterpValue::Variant {
            name: "Err".to_owned(),
            fields: vec![payload.clone()],
        }
    );
    assert_eq!(some, InterpValue::OptionSome(Box::new(payload)));
}

#[test]
fn option_and_result_unwrap_use_distinct_checked_abis() {
    let mut interner = TypeInterner::new();
    let i32_type = interner.primitive(PrimitiveType::I32);
    let string_type = interner.primitive(PrimitiveType::String);
    let option_type = interner.intern(Type::Option(i32_type));
    let result_type = interner.intern(Type::Result {
        ok: i32_type,
        err: string_type,
    });
    let store = interner.into_store();
    let projector = PureAbiProjector::build(&store).expect("checked ABI shapes should build");

    let option_call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(intrinsic::pure::OPTION_UNWRAP),
        parameter_types: vec![option_type],
        result_type: i32_type,
    };
    assert_eq!(
        execute_pure_intrinsic(
            &option_call,
            vec![InterpValue::OptionSome(Box::new(InterpValue::i32(7)))],
            &projector,
        ),
        Ok(InterpValue::i32(7))
    );
    assert_eq!(
        execute_pure_intrinsic(&option_call, vec![InterpValue::OptionNone], &projector,),
        Err(AdapterError::Builtin(BuiltinError::Abort {
            message: "unwrap encountered None".to_owned(),
        }))
    );

    let result_call = CheckedPureIntrinsicCall {
        intrinsic: StdIntrinsicId(intrinsic::pure::RESULT_UNWRAP),
        parameter_types: vec![result_type],
        result_type: i32_type,
    };
    assert_eq!(
        execute_pure_intrinsic(
            &result_call,
            vec![InterpValue::Variant {
                name: "Ok".to_owned(),
                fields: vec![InterpValue::i32(11)],
            }],
            &projector,
        ),
        Ok(InterpValue::i32(11))
    );
    assert_eq!(
        execute_pure_intrinsic(
            &result_call,
            vec![InterpValue::Variant {
                name: "Err".to_owned(),
                fields: vec![InterpValue::String("failed".to_owned())],
            }],
            &projector,
        ),
        Err(AdapterError::Builtin(BuiltinError::Abort {
            message: "unwrap encountered Err".to_owned(),
        }))
    );
}

#[test]
fn pure_builtin_arguments_project_nominal_representations_recursively() {
    let mut interner = TypeInterner::new();
    let string = interner.primitive(PrimitiveType::String);
    let bytes = interner.primitive(PrimitiveType::Bytes);
    let header_representation = interner.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "name".to_owned(),
                ty: string,
            },
            FieldType {
                name: "value".to_owned(),
                ty: string,
            },
        ],
    }));
    let header_type = interner.intern(Type::Nominal(NominalTypeRef {
        name: "HttpHeader".to_owned(),
        params: Vec::new(),
        representation: Some(header_representation),
    }));
    let headers_type = interner.intern(Type::List(header_type));
    let request_representation = interner.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "method".to_owned(),
                ty: string,
            },
            FieldType {
                name: "target".to_owned(),
                ty: string,
            },
            FieldType {
                name: "version".to_owned(),
                ty: string,
            },
            FieldType {
                name: "headers".to_owned(),
                ty: headers_type,
            },
            FieldType {
                name: "body".to_owned(),
                ty: bytes,
            },
        ],
    }));
    let request_type = interner.intern(Type::Nominal(NominalTypeRef {
        name: "HttpWireRequest".to_owned(),
        params: Vec::new(),
        representation: Some(request_representation),
    }));
    let error_type = interner.intern(Type::Enum(EnumTypeRef {
        name: "HttpCodecError".to_owned(),
    }));
    let result_type = interner.intern(Type::Result {
        ok: bytes,
        err: error_type,
    });
    let store = interner.into_store();
    let projector = PureAbiProjector::build(&store).expect("checked ABI shapes should build");
    let header = InterpValue::Nominal {
        ty: header_type,
        value: Box::new(InterpValue::Record(RecordValue::new(vec![
            (
                "name".to_owned(),
                InterpValue::String("content-length".to_owned()),
            ),
            ("value".to_owned(), InterpValue::String("5".to_owned())),
        ]))),
    };
    let request = InterpValue::Nominal {
        ty: request_type,
        value: Box::new(InterpValue::Record(RecordValue::new(vec![
            ("method".to_owned(), InterpValue::String("PUT".to_owned())),
            (
                "target".to_owned(),
                InterpValue::String("/items".to_owned()),
            ),
            (
                "version".to_owned(),
                InterpValue::String("HTTP/1.1".to_owned()),
            ),
            (
                "headers".to_owned(),
                InterpValue::List(ListValue::new(vec![header])),
            ),
            ("body".to_owned(), InterpValue::Bytes(b"hello".to_vec())),
        ]))),
    };

    let encoded = execute_pure_intrinsic(
        &CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(intrinsic::pure::HTTP_ENCODE_REQUEST),
            parameter_types: vec![request_type],
            result_type,
        },
        vec![request],
        &projector,
    )
    .expect("HTTP codec should consume nominal representations");

    assert!(matches!(encoded, InterpValue::Variant { ref name, .. } if name == "Ok"));
}

#[test]
fn pure_builtin_result_restores_nested_nominal_identity() {
    let mut interner = TypeInterner::new();
    let string = interner.primitive(PrimitiveType::String);
    let i32_type = interner.primitive(PrimitiveType::I32);
    let bytes = interner.primitive(PrimitiveType::Bytes);
    let header_representation = interner.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "name".to_owned(),
                ty: string,
            },
            FieldType {
                name: "value".to_owned(),
                ty: string,
            },
        ],
    }));
    let header_type = interner.intern(Type::Nominal(NominalTypeRef {
        name: "HttpHeader".to_owned(),
        params: Vec::new(),
        representation: Some(header_representation),
    }));
    let headers_type = interner.intern(Type::List(header_type));
    let response_representation = interner.intern(Type::Record(RecordType {
        fields: vec![
            FieldType {
                name: "version".to_owned(),
                ty: string,
            },
            FieldType {
                name: "status".to_owned(),
                ty: i32_type,
            },
            FieldType {
                name: "reason".to_owned(),
                ty: string,
            },
            FieldType {
                name: "headers".to_owned(),
                ty: headers_type,
            },
        ],
    }));
    let response_type = interner.intern(Type::Nominal(NominalTypeRef {
        name: "HttpWireResponseHead".to_owned(),
        params: Vec::new(),
        representation: Some(response_representation),
    }));
    let error_type = interner.intern(Type::Enum(EnumTypeRef {
        name: "HttpCodecError".to_owned(),
    }));
    let result_type = interner.intern(Type::Result {
        ok: response_type,
        err: error_type,
    });
    let store = interner.into_store();
    let projector = PureAbiProjector::build(&store).expect("checked ABI shapes should build");

    let decoded = execute_pure_intrinsic(
        &CheckedPureIntrinsicCall {
            intrinsic: StdIntrinsicId(intrinsic::pure::HTTP_DECODE_RESPONSE_HEAD),
            parameter_types: vec![bytes],
            result_type,
        },
        vec![InterpValue::Bytes(
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec(),
        )],
        &projector,
    )
    .expect("HTTP response head should decode");

    let InterpValue::Variant { name, fields } = decoded else {
        panic!("decode result must be a Result variant");
    };
    assert_eq!(name, "Ok");
    assert!(matches!(
        fields.as_slice(),
        [InterpValue::Nominal { ty, .. }] if *ty == response_type
    ));
}

#[test]
fn adapter_preserves_range_bounds_and_endpoints() {
    let value = InterpValue::Range(RangeValue {
        start: Box::new(InterpValue::i32(1)),
        end: Box::new(InterpValue::i32(5)),
        bounds: RangeBounds::OpenClosed,
    });

    assert_eq!(
        into_builtin(value).expect("range should convert to builtin"),
        BuiltinValue::Range {
            start: Box::new(BuiltinValue::I32(1)),
            end: Box::new(BuiltinValue::I32(5)),
            bounds: BuiltinRangeBounds::OpenClosed,
        }
    );

    assert_eq!(
        from_builtin(BuiltinValue::Range {
            start: Box::new(BuiltinValue::I32(2)),
            end: Box::new(BuiltinValue::I32(8)),
            bounds: BuiltinRangeBounds::ClosedOpen,
        })
        .expect("range should convert from builtin"),
        InterpValue::Range(RangeValue {
            start: Box::new(InterpValue::i32(2)),
            end: Box::new(InterpValue::i32(8)),
            bounds: RangeBounds::ClosedOpen,
        })
    );
}
