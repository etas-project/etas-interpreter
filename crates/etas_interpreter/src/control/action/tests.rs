use std::{collections::HashMap, sync::Arc};

use super::*;
use crate::{
    intrinsic::dispatch::{CheckedStdIntrinsicCall, StdIntrinsicIdentity},
    value::CheckedExecutionProgram,
};

fn execution(source: &str) -> CheckedExecutionProgram {
    let checked = crate::testing::project::checked_project(source);
    let plan = Arc::new(
        crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap(),
    );
    CheckedExecutionProgram {
        checked,
        plan,
        definitions: None,
        revision: None,
    }
}

fn performed(execution: &CheckedExecutionProgram) -> etas_hir::HirExprId {
    execution
        .checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Perform { .. }).then_some(id))
        .unwrap()
}

#[test]
fn source_action_origin_rejects_an_abi_or_identity_not_proven_by_the_perform() {
    let execution = execution(
        "module app.main; effect Query { action next(value: i32) -> i32; action other(value: i32) -> i32; } flow main() -> i32 ![Query.next] { return perform Query.next(4); }",
    );
    let proof =
        CheckedActionInvocation::source(execution.clone(), performed(&execution), &HashMap::new())
            .unwrap();
    proof.validate_origin().unwrap();
    let mut invalid = proof.clone();
    invalid.parameters.clear();
    assert!(
        invalid
            .validate_origin()
            .unwrap_err()
            .contains("differs from its checked origin")
    );
    let mut invalid = proof.clone();
    invalid.returns_never = true;
    assert!(invalid.validate_origin().is_err());
    let mut invalid = proof.clone();
    let etas_effects::Effect::Action(action) = &mut invalid.effect else {
        panic!("closed action")
    };
    action.action = etas_effects::EffectActionId(u32::MAX);
    assert!(invalid.validate_origin().is_err());
    let mut invalid = proof;
    invalid.output = TypeId(u32::MAX);
    assert!(invalid.validate_origin().is_err());
    assert!(
        CheckedActionInvocation::source(execution, etas_hir::HirExprId(u32::MAX), &HashMap::new())
            .unwrap_err()
            .contains("not a checked perform")
    );
}

#[test]
fn source_action_origin_keeps_instantiated_generic_selector_and_payload_types() {
    let execution = execution(
        "module app.main; effect Query { action choose<T>(value: T) -> T; } flow generic<T>(value: T) -> T ![Query.choose<T>] { return perform Query.choose<T>(value); } flow main() -> i32 ![Query.choose<i32>] { return generic<i32>(4); }",
    );
    let integer = execution
        .checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            (*ty == etas_types::Type::Primitive(etas_types::PrimitiveType::I32)).then_some(id)
        })
        .unwrap();
    let bindings = HashMap::from([("T".into(), integer)]);
    let proof =
        CheckedActionInvocation::source(execution.clone(), performed(&execution), &bindings)
            .unwrap();
    assert_eq!(proof.parameters, [integer]);
    assert_eq!(proof.output, integer);
    let etas_effects::Effect::AppliedAction(action) = &proof.effect else {
        panic!("generic selector")
    };
    assert_eq!(action.args, [etas_types::EffectArgRef::Type(integer)]);
    proof.validate_origin().unwrap();
    let invalid = HashMap::from([("T".into(), TypeId(u32::MAX))]);
    assert!(
        CheckedActionInvocation::source(execution.clone(), performed(&execution), &invalid)
            .is_err()
    );
}

#[test]
fn source_action_origin_rejects_generic_bindings_that_violate_action_spec_bounds() {
    let execution = execution(
        "module app.main; spec Allowed; type User = string; impl User ~ Allowed; effect Query { action choose<T ~ Allowed>(value: T) -> T; } flow generic<T ~ Allowed>(value: T) -> T ![Query.choose<T>] { return perform Query.choose<T>(value); } flow main() -> User ![Query.choose<User>] { let unrelated: i32 = 1; return generic<User>(User(\"allowed\")); }",
    );
    let signature = execution.checked.types.item_signatures[&execution.plan.entry.item].clone();
    let etas_types::ItemSignature::Flow(signature) = signature else {
        panic!("entry flow")
    };
    let expr = performed(&execution);
    let bindings = HashMap::from([("T".into(), signature.output)]);
    CheckedActionInvocation::source(execution.clone(), expr, &bindings)
        .unwrap()
        .validate_origin()
        .unwrap();
    let integer = execution
        .checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            (*ty == etas_types::Type::Primitive(etas_types::PrimitiveType::I32)).then_some(id)
        })
        .unwrap();
    let bindings = HashMap::from([("T".into(), integer)]);
    let error = CheckedActionInvocation::source(execution.clone(), expr, &bindings).unwrap_err();
    assert!(error.contains("violates a checked spec bound"), "{error}");
    let error = CheckedActionInvocation::source(execution, expr, &HashMap::new()).unwrap_err();
    assert!(
        error.contains("missing a bounded generic argument"),
        "{error}"
    );
}

#[test]
fn inference_action_origin_cannot_change_the_checked_schema_or_output() {
    let execution = execution(
        "module app.main; agent Worker() -> i32 { return perform infer<i32>(Prompt.new()); } agent Text() -> string { return perform infer<string>(Prompt.new()); } flow main() -> i32 { return Worker(); }",
    );
    let (expr, inference) = execution
        .checked
        .types
        .agent_inferences
        .iter()
        .find(|(_, fact)| {
            execution.checked.type_store.get(fact.output)
                == Some(&etas_types::Type::Primitive(etas_types::PrimitiveType::I32))
        })
        .unwrap();
    let signature = &execution.checked.types.action_signatures[&inference.action_symbol];
    let [parameter] = signature.generic_params.as_slice() else {
        panic!("single inference output parameter")
    };
    let bindings = HashMap::from([(parameter.name.clone(), inference.output)]);
    CheckedActionInvocation::source(execution.clone(), *expr, &bindings)
        .unwrap()
        .validate_origin()
        .unwrap();
    let string = execution
        .checked
        .type_store
        .iter()
        .find_map(|(id, ty)| {
            (*ty == etas_types::Type::Primitive(etas_types::PrimitiveType::String)).then_some(id)
        })
        .unwrap();
    let bindings = HashMap::from([(parameter.name.clone(), string)]);
    let error = CheckedActionInvocation::source(execution.clone(), *expr, &bindings).unwrap_err();
    assert!(error.contains("payload/output facts"), "{error}");
}

fn console_call(execution: &CheckedExecutionProgram) -> CheckedStdIntrinsicCall {
    let (origin, args) = execution
        .checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            etas_hir::HirExpr::Call { args, .. } => Some((id, args)),
            _ => None,
        })
        .unwrap();
    CheckedStdIntrinsicCall {
        identity: StdIntrinsicIdentity {
            intrinsic: etas_std::StdIntrinsicId(etas_std::intrinsic::runtime::IO_PRINTLN),
            dispatch: etas_std::IntrinsicDispatch::Runtime,
        },
        origin,
        substitutions: Vec::new(),
        parameter_types: args
            .iter()
            .map(|arg| {
                let (etas_hir::HirArg::Positional(value) | etas_hir::HirArg::Named { value, .. }) =
                    arg;
                execution.checked.types.expr_types[value]
            })
            .collect(),
        result_type: execution.checked.types.expr_types[&origin],
    }
}

#[test]
fn standard_action_origin_requires_the_current_descriptor_and_exact_checked_call_abi() {
    let execution = execution(
        "module app.main; import std.io.println; flow main() -> unit { println(\"hello\"); return; }",
    );
    let call = console_call(&execution);
    let proof = CheckedActionInvocation::standard(execution.clone(), &call).unwrap();
    assert_eq!(
        proof.effect,
        etas_effects::Effect::Action(etas_effects::ActionRef {
            tag: etas_effects::CONSOLE_TAG,
            action: etas_effects::CONSOLE_STDOUT_WRITE_ACTION
        })
    );
    proof.validate_origin().unwrap();
    let mut invalid = call.clone();
    invalid.origin = etas_hir::HirExprId(u32::MAX);
    assert!(
        CheckedActionInvocation::standard(execution.clone(), &invalid)
            .unwrap_err()
            .contains("no checked descriptor binding")
    );
    let mut invalid = call.clone();
    invalid.result_type = call.parameter_types[0];
    assert!(
        CheckedActionInvocation::standard(execution.clone(), &invalid)
            .unwrap_err()
            .contains("ABI differs")
    );
    let mut invalid = call;
    invalid.identity.intrinsic =
        etas_std::StdIntrinsicId(etas_std::intrinsic::runtime::IO_EPRINTLN);
    assert!(CheckedActionInvocation::standard(execution, &invalid).is_err());
}

#[test]
fn action_evidence_aliases_share_the_native_execution_instead_of_copying_the_type_store() {
    let execution = execution(
        "module app.main; effect Query { action next(value: i32) -> i32; } flow main() -> i32 ![Query.next] { return perform Query.next(4); }",
    );
    let proof =
        CheckedActionInvocation::source(execution.clone(), performed(&execution), &HashMap::new())
            .unwrap();
    let (_, cost) = crate::testing::allocation::measure(|| {
        let copies = (0..100).map(|_| proof.clone()).collect::<Vec<_>>();
        assert!(
            copies
                .iter()
                .all(|copy| Arc::ptr_eq(&copy.program, &execution.checked))
        );
    });
    assert!(
        cost.bytes < 32_000,
        "invocation copies duplicated checked program state: {cost:?}"
    );
    proof.validate_origin().unwrap();
}
