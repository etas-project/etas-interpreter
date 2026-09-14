use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

#[test]
fn member_assignment_rejects_missing_checked_projection_before_mutation() {
    let checked = crate::testing::project::checked_project(
        "module app.main; type Row = { value: string } flow main() -> unit { var row = Row { value = \"old\" }; row.value = \"new\"; return; }",
    );
    let mut plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    // Corrupt only the prepared projection table, leaving valid typed source in place.
    plan.records = Default::default();
    let symbol = checked.symbols.iter().find(|s| s.name == "row").unwrap().id;
    let span = checked.symbols.get(symbol).unwrap().definition_span;
    let target = checked
        .hir
        .stmts
        .iter()
        .find_map(|(_, stmt)| match stmt {
            HirStmt::Assign { target, .. } => Some(*target),
            _ => None,
        })
        .unwrap();
    let options = RunOptions::default();
    let mut eval = EvalContext::new(EvalContextInput {
        storage_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        checked: &checked,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: None,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
    });
    let mut frame = Frame::new(plan.slots.clone());
    let original = InterpValue::Record(crate::value::RecordValue::new(vec![(
        "value".into(),
        InterpValue::String("old".into()),
    )]));
    frame.insert(symbol, original.clone());
    let result = eval.assign_target_with_resume(
        target,
        InterpValue::String("new".into()),
        &mut frame,
        span,
        None,
    );
    let Err(signal) = result else {
        panic!("missing checked projection must fail")
    };
    let ControlSignal::Fault(fault) = *signal else {
        panic!("structured execution fault")
    };
    assert!(fault.message.contains("no checked record projection"));
    assert_eq!(frame.get(symbol), Some(original));
}

#[test]
fn assignment_commit_reuses_unique_local_and_copies_only_live_aliases() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { var root = [\"old\"]; var nested = [[\"old\"]]; var scalar = \"old\"; scalar = \"new\"; return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let symbol = checked
        .symbols
        .iter()
        .find(|s| s.name == "root")
        .unwrap()
        .id;
    let span = checked.symbols.get(symbol).unwrap().definition_span;
    let options = RunOptions::default();
    let mut eval = EvalContext::new(EvalContextInput {
        storage_limits: Default::default(),
        event_observer: None,
        execution: etas_host::execution::ExecutionScope::new_owned(),
        checked: &checked,
        plan: &plan,
        host_context: options.host_context,
        model_policy: options.model_policy,
        execution_limits: options.execution_limits,
        consumed_steps: 0,
        current_session: None,
        entry_item: checked.entry.unwrap(),
        entry_args: &[],
    });
    let scalar = checked
        .symbols
        .iter()
        .find(|s| s.name == "scalar")
        .unwrap()
        .id;
    let target = checked
        .hir
        .stmts
        .iter()
        .find_map(|(_, stmt)| match stmt {
            HirStmt::Assign { target, .. } => Some(*target),
            _ => None,
        })
        .unwrap();
    let mut frame = Frame::new(plan.slots.clone());
    frame.insert(scalar, InterpValue::String("old".into()));
    let replacement = "new".repeat(4096);
    let pointer = replacement.as_ptr();
    let replacement = InterpValue::String(replacement.into());
    let (result, allocations) =
        measure(|| eval.assign_target_with_resume(target, replacement, &mut frame, span, None));
    result.unwrap();
    assert_eq!(
        allocations.count, 0,
        "the resolved target must retain the owned RHS"
    );
    frame
        .with_local_mut(scalar, |value| {
            let InterpValue::String(value) = value else {
                panic!("expected string")
            };
            assert_eq!(value.as_ptr(), pointer);
        })
        .unwrap();
    for count in [1000, 2000, 4000] {
        for restored in [false, true] {
            let value = InterpValue::Array(ArrayValue::new(
                (0..count)
                    .map(|_| InterpValue::String("x".repeat(128).into()))
                    .collect(),
            ));
            let mut frame = if restored {
                Frame::from_snapshot(vec![(symbol, value)]).unwrap()
            } else {
                let mut frame = Frame::new(plan.slots.clone());
                frame.insert(symbol, value);
                frame
            };
            let observer = frame.clone();
            let segments = [LocalPlaceSegment::Index(count - 1)];
            for _ in 0..3 {
                let replacement = InterpValue::String("replacement".repeat(32).into());
                let (result, allocations) = measure(|| {
                    eval.assign_resolved_local_place(
                        symbol,
                        &segments,
                        replacement,
                        &mut frame,
                        span,
                    )
                });
                result.unwrap();
                assert_eq!(
                    allocations.count, 0,
                    "unique commit, n={count}, restored={restored}"
                );
            }
            // A continuation's frame reference is not a language-level value alias.
            assert_eq!(observer.get(symbol), frame.get(symbol));
            let alias = frame.get(symbol).unwrap();
            let replacement = InterpValue::String("new".into());
            let (result, allocations) = measure(|| {
                eval.assign_resolved_local_place(symbol, &segments, replacement, &mut frame, span)
            });
            result.unwrap();
            assert_eq!(
                allocations.count, 2,
                "COW vector and backing, strings remain shared"
            );
            assert!(allocations.bytes >= count * std::mem::size_of::<InterpValue>());
            assert_ne!(Some(&alias), frame.get(symbol).as_ref());
            let before_error = frame.get(symbol).unwrap();
            assert!(
                eval.assign_resolved_local_place(
                    symbol,
                    &[LocalPlaceSegment::Index(count)],
                    InterpValue::Unit,
                    &mut frame,
                    span,
                )
                .is_err()
            );
            assert_eq!(frame.get(symbol), Some(before_error));
        }
        let nested = checked
            .symbols
            .iter()
            .find(|s| s.name == "nested")
            .unwrap()
            .id;
        let child = InterpValue::Array(ArrayValue::new(
            (0..count)
                .map(|_| InterpValue::String("x".repeat(128).into()))
                .collect(),
        ));
        let mut frame = Frame::new(plan.slots.clone());
        frame.insert(nested, InterpValue::Array(ArrayValue::new(vec![child])));
        let segments = [
            LocalPlaceSegment::Index(0),
            LocalPlaceSegment::Index(count - 1),
        ];
        let replacement = InterpValue::String("new".into());
        let (result, allocations) = measure(|| {
            eval.assign_resolved_local_place(nested, &segments, replacement, &mut frame, span)
        });
        result.unwrap();
        assert_eq!(allocations.count, 0, "unique nested commit, n={count}");
        let alias = frame.get(nested).unwrap();
        assert!(
            eval.assign_resolved_local_place(
                nested,
                &[LocalPlaceSegment::Index(0), LocalPlaceSegment::Index(count)],
                InterpValue::Unit,
                &mut frame,
                span,
            )
            .is_err()
        );
        assert_eq!(frame.get(nested), Some(alias));
    }
}
