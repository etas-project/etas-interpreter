use super::*;
use crate::value::{AgentRevisionValue, InterpValue, RecordValue};
use etas_code::descriptor::GenericOwner;

const NOMINAL_SOURCE: &str = "module app; type UserId = string; type Profile = { id: UserId }; agent Worker(value: UserId) -> Profile { return Profile { id = value }; } flow inspect() -> unit { let d = ^Worker; return; }";

#[test]
fn sibling_revisions_compose_checked_nominal_argument_and_result_bridges() {
    let base = frozen(NOMINAL_SOURCE);
    let first = InterpreterCandidatePublisher
        .publish(compile(base.clone(), "return Profile { id = value };"))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(
            base,
            "let extra: Option<i32> = None; return Profile { id = value };",
        ))
        .unwrap();
    assert!(!Arc::ptr_eq(first.program(), second.program()));
    let component = first
        .type_bridge()
        .to_candidate(
            first.type_bridge().original_program(),
            &GenericOwner::Declaration(second.identity().clone()),
            second.targets.agent_component().unwrap(),
        )
        .unwrap();
    let value = AgentRevisionValue::new_in_program(
        second.clone(),
        Some(first.clone()),
        first.program(),
        component,
    )
    .unwrap();
    let target = value.target().unwrap();
    let source_param = first
        .type_bridge()
        .to_candidate(
            first.type_bridge().original_program(),
            &target.scope,
            target.original.params[0],
        )
        .unwrap();
    let arg = value
        .argument_to_revision(
            first.program(),
            target.original.params[0],
            InterpValue::Nominal {
                ty: source_param,
                value: InterpValue::String("u1".into()).into(),
            },
            None,
        )
        .unwrap();
    assert!(matches!(&arg, InterpValue::Nominal { ty, .. } if *ty == target.candidate.params[0]));
    let result = value
        .result_to_source(
            second.program(),
            InterpValue::Nominal {
                ty: target.candidate.output,
                value: InterpValue::Record(RecordValue::new(vec![("id".into(), arg)])).into(),
            },
            None,
        )
        .unwrap();
    let expected = first
        .type_bridge()
        .to_candidate(
            first.type_bridge().original_program(),
            &target.scope,
            target.original.output,
        )
        .unwrap();
    let InterpValue::Nominal {
        ty,
        value: representation,
    } = result
    else {
        panic!("nominal result")
    };
    assert_eq!(ty, expected);
    let InterpValue::Record(fields) = representation.as_ref() else {
        panic!("record representation")
    };
    assert!(
        matches!(fields.get("id"), Some(InterpValue::Nominal { ty, .. }) if ty == source_param)
    );
    let id = value.source_agent().unwrap();
    let body = first
        .definitions
        .definitions()
        .catalog()
        .item_owner(&first.targets.default)
        .unwrap();
    let method = value.select_method(first.program(), id, body).unwrap();
    assert_eq!(method, value);
    assert_eq!(
        method.target().unwrap().candidate_item,
        second.targets.callables[&second.targets.default].candidate_item
    );
}

#[test]
fn identical_descriptors_do_not_authorize_a_bridge_between_unrelated_programs() {
    let first = InterpreterCandidatePublisher
        .publish(compile(
            frozen(NOMINAL_SOURCE),
            "return Profile { id = value };",
        ))
        .unwrap();
    let second = InterpreterCandidatePublisher
        .publish(compile(
            frozen(NOMINAL_SOURCE),
            "return Profile { id = value };",
        ))
        .unwrap();
    assert_eq!(first.identity(), second.identity());
    assert_eq!(
        first.targets.agent_component().unwrap(),
        second.targets.agent_component().unwrap()
    );
    let result = AgentRevisionValue::new_in_program(
        second.clone(),
        Some(first.clone()),
        first.program(),
        first.targets.agent_component().unwrap(),
    );
    assert!(
        result
            .unwrap_err()
            .contains("no common checked source universe")
    );
}
