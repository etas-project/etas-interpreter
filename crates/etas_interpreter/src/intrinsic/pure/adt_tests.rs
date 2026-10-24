use super::*;
use crate::{
    intrinsic::dispatch::CheckedPureIntrinsicCall, testing::allocation::measure, value::InterpValue,
};
use etas_std::{StdIntrinsicId, intrinsic::pure};
use etas_types::{NominalTypeRef, PrimitiveType, Type, TypeInterner};

#[test]
fn checked_adt_wrap_tag_and_unwrap_share_nominal_payloads() {
    for count in [1000, 2000, 4000] {
        let mut types = TypeInterner::new();
        let text = types.primitive(PrimitiveType::String);
        let tuple = types.intern(Type::Tuple(vec![text; count]));
        let nominal = types.intern(Type::Nominal(NominalTypeRef {
            name: "Wide".into(),
            representation: Some(tuple),
            params: vec![],
        }));
        let boolean = types.primitive(PrimitiveType::Bool);
        let option = types.intern(Type::Option(nominal));
        let result = types.intern(Type::Result {
            ok: nominal,
            err: nominal,
        });
        let projector = PureAbiProjector::build(&types.into_store()).unwrap();
        let fields = crate::value::SharedFields::new(
            (0..count)
                .map(|_| InterpValue::String("payload".repeat(128).into()))
                .collect(),
        );
        let pointer = fields.as_ptr();
        let original = InterpValue::Nominal {
            ty: nominal,
            value: InterpValue::Tuple(fields).into(),
        };
        for (wrap_id, tag_id, wrapper, unwrap_id, max_allocations) in [
            (
                pure::OPTION_SOME,
                pure::OPTION_IS_SOME,
                option,
                Some(pure::OPTION_UNWRAP),
                1,
            ),
            (
                pure::RESULT_OK,
                pure::RESULT_IS_OK,
                result,
                Some(pure::RESULT_UNWRAP),
                4,
            ),
            (pure::RESULT_ERR, pure::RESULT_IS_ERR, result, None, 4),
        ] {
            let wrap = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(wrap_id),
                parameter_types: vec![nominal],
                result_type: wrapper,
            };
            let args = vec![original.clone()];
            let (wrapped, cost) =
                measure(|| execute_pure_intrinsic(&wrap, args, &projector).unwrap());
            assert_eq!(
                cost.count, max_allocations,
                "wrapper headers only, n={count}: {cost:?}"
            );
            assert!(cost.bytes < count);
            let tag = CheckedPureIntrinsicCall {
                intrinsic: StdIntrinsicId(tag_id),
                parameter_types: vec![wrapper],
                result_type: boolean,
            };
            let args = vec![wrapped.clone()];
            let (tag, cost) = measure(|| execute_pure_intrinsic(&tag, args, &projector).unwrap());
            assert_eq!(tag, InterpValue::Bool(true));
            assert_eq!(cost.count, 0);
            if let Some(unwrap_id) = unwrap_id {
                let unwrap = CheckedPureIntrinsicCall {
                    intrinsic: StdIntrinsicId(unwrap_id),
                    parameter_types: vec![wrapper],
                    result_type: nominal,
                };
                for args in [vec![wrapped.clone()], vec![wrapped]] {
                    let (payload, cost) =
                        measure(|| execute_pure_intrinsic(&unwrap, args, &projector).unwrap());
                    assert_eq!(
                        cost.count, 0,
                        "unwrap must not copy wide ADTs, n={count}: {cost:?}"
                    );
                    let InterpValue::Nominal { ty, value } = payload else {
                        panic!("nominal")
                    };
                    assert_eq!(ty, nominal);
                    let InterpValue::Tuple(fields) = value.as_ref() else {
                        panic!("tuple")
                    };
                    assert_eq!(fields.as_ptr(), pointer);
                }
            }
        }
    }
}
