use super::*;
use crate::testing::allocation::measure;

fn with_validator(test: impl FnOnce(&SnapshotValidator<'_>, HirItemId)) {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> unit { return; }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let limits = etas_host::StorageLimits::default();
    let validator = SnapshotValidator::new(
        &checked,
        &plan.slots,
        &plan.dispatch,
        &plan.closures,
        &limits,
    );
    test(&validator, checked.entry.unwrap());
}

fn target(item: HirItemId, depth: usize, composed: bool) -> CallTargetSnapshot {
    let mut value = CallTargetSnapshot::FlowItem(item);
    for _ in 0..depth {
        value = if composed {
            CallTargetSnapshot::Composed(vec![value].into())
        } else {
            CallTargetSnapshot::Limited {
                target: value.into(),
                limits: vec![],
            }
        };
    }
    value
}

#[test]
fn shared_callable_validation_visits_one_graph_across_separate_roots() {
    for composed in [false, true] {
        for count in [1000, 2000, 4000] {
            with_validator(|validator, item| {
                let saved = target(item, 128, composed);
                let retained = saved.clone();
                call_target::VISITS.set(0);
                let (_, cost) = measure(|| {
                    for _ in 0..count {
                        validator.call_target(&saved, "shared callable").unwrap();
                    }
                });
                let visits = call_target::VISITS.get();
                eprintln!(
                    "shared callable validation count={count} composed={composed}: visits={visits} {cost:?}"
                );
                assert_eq!(visits, count + 128, "repeated shared subtree validation");
                assert!(cost.count < 32, "per-root allocation: {cost:?}");
                drop(retained);
            });
        }
    }
}

#[test]
fn failed_frame_validation_is_not_cached_as_success() {
    with_validator(|validator, _| {
        let invalid = LocalsSnapshot {
            id: 1,
            locals: Default::default(),
            type_bindings: vec![("T".into(), TypeId(u32::MAX))],
        };
        for context in ["first frame", "second frame"] {
            let error = validator.frame(&invalid, context).unwrap_err();
            assert!(error.contains("missing checked type"), "{error}");
            assert!(error.starts_with(context), "{error}");
        }
    });
}

#[test]
fn shared_callable_validation_cache_preserves_cow_and_outer_binding_errors() {
    with_validator(|validator, item| {
        for composed in [false, true] {
            let mut changed = target(item, 128, composed);
            let retained = changed.clone();
            validator.call_target(&changed, "original").unwrap();
            let mut cursor = &mut changed;
            for _ in 0..128 {
                cursor = match cursor {
                    CallTargetSnapshot::Limited { target, .. } => target,
                    CallTargetSnapshot::Composed(targets) => &mut targets[0],
                    _ => panic!("target chain"),
                };
            }
            *cursor = CallTargetSnapshot::FlowItem(HirItemId(u32::MAX));
            for context in ["changed first", "changed again"] {
                let error = validator.call_target(&changed, context).unwrap_err();
                assert!(error.starts_with(context), "{error}");
                assert!(error.contains("missing HIR item"), "{error}");
            }
            call_target::VISITS.set(0);
            validator.call_target(&retained, "retained").unwrap();
            assert_eq!(call_target::VISITS.get(), 1);
            for (name, expected) in [
                ("", "invalid or duplicate type parameter"),
                ("T", "missing checked type"),
            ] {
                let wrapper = CallTargetSnapshot::Specialized {
                    target: retained.clone().into(),
                    type_bindings: vec![(name.into(), TypeId(u32::MAX))],
                };
                for context in ["binding first", "binding again"] {
                    let error = validator.call_target(&wrapper, context).unwrap_err();
                    assert!(error.starts_with(context), "{error}");
                    assert!(error.contains(expected), "{error}");
                }
            }
        }
    });
}

#[test]
fn failed_shared_callable_parent_is_rechecked_after_completed_siblings() {
    with_validator(|validator, item| {
        for composed in [false, true] {
            let good = target(item, 128, composed);
            let invalid_binding = CallTargetSnapshot::Specialized {
                target: good.clone().into(),
                type_bindings: vec![("".into(), TypeId(u32::MAX))],
            };
            for (children, expected) in [
                (
                    vec![
                        good.clone(),
                        CallTargetSnapshot::FlowItem(HirItemId(u32::MAX)),
                        invalid_binding.clone(),
                    ],
                    "missing HIR item",
                ),
                (
                    vec![
                        good.clone(),
                        invalid_binding,
                        CallTargetSnapshot::FlowItem(HirItemId(u32::MAX)),
                    ],
                    "invalid or duplicate type parameter",
                ),
            ] {
                let bad = CallTargetSnapshot::Composed(children.into());
                let retained = bad.clone();
                for context in ["first parent", "second parent"] {
                    let error = validator.call_target(&bad, context).unwrap_err();
                    assert!(error.starts_with(context), "{error}");
                    assert!(error.contains(expected), "{error}");
                }
                assert!(bad == retained);
            }
        }
    });
}

#[test]
fn callable_validation_cache_lifetime_releases_sources_and_preserves_new_identities() {
    with_validator(|original, item| {
        let (_, cost) = measure(|| {
            let validator = SnapshotValidator::new(
                original.checked,
                original.slots,
                original.dispatch,
                original.closures,
                original.limits,
            );
            for composed in [false, true] {
                for _ in 0..64 {
                    let saved = target(item, 128, composed);
                    let retained = saved.clone();
                    validator.call_target(&saved, "transient source").unwrap();
                    drop((saved, retained));
                    let bad = target(HirItemId(u32::MAX), 128, composed);
                    assert!(
                        validator
                            .call_target(&bad, "new invalid source")
                            .unwrap_err()
                            .contains("missing HIR item")
                    );
                }
            }
        });
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "retained source owners leaked: {cost:?}"
        );
    });
}

#[test]
fn checked_machine_validation_reuses_callable_graph_but_rejects_late_tampering() {
    with_validator(|validator, item| {
        let span = validator.checked.hir.blocks.iter().next().unwrap().1.span;
        for count in [1000, 2000, 4000] {
            let continuation = ContinuationSnapshot::CallArgs {
                target: target(item, 128, true),
                args: vec![].into(),
                next_arg_index: 0,
                evaluated_args: vec![],
                span,
                frame: LocalsSnapshot {
                    id: 1,
                    locals: Default::default(),
                    type_bindings: vec![],
                },
            };
            let mut machine = MachineSnapshot {
                frames: (0..count)
                    .map(|_| MachineFrameSnapshot::Continuation {
                        continuation: continuation.clone(),
                    })
                    .collect(),
            };
            call_target::VISITS.set(0);
            validator.validate_machine(&machine).unwrap();
            assert_eq!(call_target::VISITS.get(), count + 128);
            let MachineFrameSnapshot::Continuation {
                continuation: ContinuationSnapshot::CallArgs { target, .. },
            } = machine.frames.last_mut().unwrap()
            else {
                panic!("call frame")
            };
            *target = CallTargetSnapshot::FlowItem(HirItemId(u32::MAX));
            for _ in 0..2 {
                let error = validator.validate_machine(&machine).unwrap_err();
                assert!(
                    error.starts_with(&format!("machine frame {}", count - 1)),
                    "{error}"
                );
                assert!(error.contains("missing HIR item"), "{error}");
            }
        }
    });
}
