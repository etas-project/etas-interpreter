use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

fn with_collection_eval(run: impl FnOnce(&mut EvalContext<'_>, HirExprId, Span)) {
    with_checked_collection_eval(
        "module app.main; flow main() -> Array<string> { return [\"start\"].push(\"end\"); }",
        run,
    );
}

fn with_checked_collection_eval(
    source: &str,
    run: impl FnOnce(&mut EvalContext<'_>, HirExprId, Span),
) {
    let checked = crate::testing::project::checked_project(source);
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let (expr, span) = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, expr)| match expr {
            HirExpr::MethodCall { span, .. } => Some((id, *span)),
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
    run(&mut eval, expr, span);
}

#[test]
fn checked_list_at_does_not_allocate_on_successful_lookup() {
    with_checked_collection_eval(
        "module app.main; flow main() -> Result<string, IndexError> { return [\"start\"; \"end\"].at(0)?; }",
        |eval, expr, span| {
            assert!(eval.checked.types.checked_index_errors.contains_key(&expr));
            for count in [1000, 2000, 4000] {
                let list = crate::value::ListValue::new(
                    (0..count)
                        .map(|_| InterpValue::String("payload".repeat(128).into()))
                        .collect(),
                );
                for index in [0, count / 2, count - 1] {
                    let args = vec![InterpValue::i32(i32::try_from(index).unwrap())];
                    let (value, cost) = measure(|| {
                        eval.eval_local_method_with_values(
                            expr,
                            InterpValue::List(list.clone()),
                            "at",
                            &[],
                            args,
                            span,
                        )
                    });
                    assert_eq!(
                        cost.count, 0,
                        "at must borrow cells: n={count}, index={index}, {cost:?}"
                    );
                    let ControlSignal::Value(InterpValue::String(value)) = value else {
                        panic!("at string")
                    };
                    let InterpValue::String(original) = list.get(index).unwrap() else {
                        panic!("string")
                    };
                    assert_eq!(value.as_ptr(), original.as_ptr());
                }
            }
        },
    );
}

#[test]
fn list_get_borrows_cells_and_clones_only_the_selected_payload() {
    with_collection_eval(|eval, expr, span| {
        for count in [1000, 2000, 4000] {
            let list = crate::value::ListValue::new(
                (0..count)
                    .map(|_| InterpValue::String("payload".repeat(128).into()))
                    .collect(),
            );
            for index in [0, count / 2, count - 1, count] {
                let args = vec![InterpValue::usize(index)];
                let (result, cost) = measure(|| {
                    eval.eval_local_method_with_values(
                        expr,
                        InterpValue::List(list.clone()),
                        "get",
                        &[],
                        args,
                        span,
                    )
                });
                assert_eq!(
                    cost.count,
                    usize::from(index < count),
                    "only Some wrapper, no list snapshot: n={count}, index={index}, {cost:?}"
                );
                match result {
                    ControlSignal::Value(InterpValue::OptionSome(value)) => {
                        let (InterpValue::String(old), InterpValue::String(new)) =
                            (list.get(index).unwrap(), &*value)
                        else {
                            panic!("string payload")
                        };
                        assert_eq!(old.as_ptr(), new.as_ptr());
                    }
                    ControlSignal::Value(InterpValue::OptionNone) => assert_eq!(index, count),
                    other => panic!("unexpected get result: {other:?}"),
                }
                assert_eq!(list.len(), count);
            }
        }
    });
}

#[test]
fn list_tail_and_extend_share_suffix_without_payload_materialization() {
    with_collection_eval(|eval, expr, span| {
        for count in [1000, 2000, 4000] {
            let suffix = crate::value::ListValue::new(
                (0..count)
                    .map(|_| InterpValue::String("suffix".repeat(128).into()))
                    .collect(),
            );
            let (tail, cost) = measure(|| {
                eval.eval_local_method_with_values(
                    expr,
                    InterpValue::List(suffix.clone()),
                    "tail",
                    &[],
                    vec![],
                    span,
                )
            });
            assert_eq!(
                cost.count, 1,
                "tail allocates only Some wrapper: n={count}, {cost:?}"
            );
            let ControlSignal::Value(InterpValue::OptionSome(tail)) = tail else {
                panic!("tail")
            };
            let InterpValue::List(tail) = &*tail else {
                panic!("list tail")
            };
            assert!(std::ptr::eq(tail.get(0).unwrap(), suffix.get(1).unwrap()));
            for shared in [false, true] {
                let prefix = crate::value::ListValue::new(
                    (0..count)
                        .map(|_| InterpValue::String("prefix".repeat(128).into()))
                        .collect(),
                );
                let alias = shared.then(|| prefix.clone());
                let args = vec![InterpValue::List(prefix)];
                let (joined, cost) = measure(|| {
                    eval.eval_local_method_with_values(
                        expr,
                        InterpValue::List(suffix.clone()),
                        "extend",
                        &[],
                        args,
                        span,
                    )
                });
                assert_eq!(
                    cost.count,
                    if shared { count } else { 0 },
                    "extend copies shared prefix cells only: n={count}, shared={shared}, {cost:?}"
                );
                let ControlSignal::Value(InterpValue::List(joined)) = joined else {
                    panic!("joined")
                };
                assert_eq!(joined.len(), 2 * count);
                assert!(std::ptr::eq(
                    joined.get(count).unwrap(),
                    suffix.get(0).unwrap()
                ));
                if let Some(alias) = alias {
                    assert_eq!(alias.len(), count);
                    let (InterpValue::String(old), InterpValue::String(new)) =
                        (alias.get(0).unwrap(), joined.get(0).unwrap())
                    else {
                        panic!("string payload")
                    };
                    assert_eq!(
                        old.as_ptr(),
                        new.as_ptr(),
                        "shared cells retain immutable payload backing"
                    );
                }
            }
        }
    });
}

#[test]
fn collection_kernels_move_owned_arguments_and_preserve_live_aliases() {
    with_collection_eval(|eval, expr, span| {
        for count in [1000, 2000, 4000] {
            let tail = crate::value::ListValue::new(
                (0..count)
                    .map(|_| InterpValue::String("tail".repeat(128).into()))
                    .collect(),
            );
            let receiver = InterpValue::List(tail.clone());
            let args = vec![InterpValue::String("head".repeat(128).into())];
            let (pushed, cost) = measure(|| {
                eval.eval_local_method_with_values(expr, receiver, "push", &[], args, span)
            });
            assert_eq!(
                cost.count, 1,
                "List kernel must cons rather than materialize, n={count}"
            );
            let ControlSignal::Value(InterpValue::List(pushed)) = pushed else {
                panic!("List")
            };
            assert!(std::ptr::eq(pushed.get(1).unwrap(), tail.get(0).unwrap()));
            let (popped, cost) = measure(|| {
                eval.eval_local_method_with_values(
                    expr,
                    InterpValue::List(pushed),
                    "pop",
                    &[],
                    vec![],
                    span,
                )
            });
            assert_eq!(
                cost.count, 3,
                "pop allocates tuple slots, shared tuple header and Some header only, n={count}"
            );
            let ControlSignal::Value(InterpValue::Tuple(popped)) = popped else {
                panic!("pop tuple")
            };
            assert_eq!(popped[0], InterpValue::List(tail));

            let mut left = Vec::with_capacity(2 * count);
            left.extend((0..count).map(|_| InterpValue::String("left".repeat(128).into())));
            let left = InterpValue::Array(ArrayValue::new(left));
            let right = ArrayValue::new(
                (0..count)
                    .map(|_| InterpValue::String("right".repeat(128).into()))
                    .collect(),
            );
            let pointer = match &right.borrow()[0] {
                InterpValue::String(s) => s.as_ptr(),
                _ => panic!("string"),
            };
            let args = vec![InterpValue::Array(right)];
            let (result, allocations) = measure(|| {
                eval.eval_local_method_with_values(expr, left, "extend", &[], args, span)
            });
            assert_eq!(
                allocations.count, 0,
                "reuse unique backing with sufficient capacity, n={count}: {allocations:?}"
            );
            let ControlSignal::Value(InterpValue::Array(result)) = result else {
                panic!("array result")
            };
            assert_eq!(result.borrow().len(), 2 * count);
            let values = result.borrow();
            let InterpValue::String(first_right) = &values[count] else {
                panic!("string")
            };
            assert_eq!(first_right.as_ptr(), pointer);

            let alias = result.clone();
            let rhs = "owned argument".repeat(128);
            let pointer = rhs.as_ptr();
            let args = vec![InterpValue::String(rhs.into())];
            let (result, allocations) = measure(|| {
                eval.eval_local_method_with_values(
                    expr,
                    InterpValue::Array(result),
                    "push",
                    &[],
                    args,
                    span,
                )
            });
            assert_eq!(
                allocations.count, 2,
                "one final COW vector and backing; no intermediate growth buffer"
            );
            assert!(allocations.bytes >= 2 * count * std::mem::size_of::<InterpValue>());
            let ControlSignal::Value(InterpValue::Array(result)) = result else {
                panic!("array result")
            };
            assert_eq!(alias.borrow().len(), 2 * count);
            assert_eq!(result.borrow().len(), 2 * count + 1);
            let values = result.borrow();
            let InterpValue::String(last) = &values[2 * count] else {
                panic!("string")
            };
            assert_eq!(
                last.as_ptr(),
                pointer,
                "COW receiver must not copy the independent owned argument"
            );
        }
        for (method, args) in [
            ("push", vec![]),
            ("len", vec![InterpValue::Unit]),
            ("unknown", vec![]),
        ] {
            assert!(matches!(
                eval.eval_local_method_with_values(
                    expr,
                    InterpValue::Array(ArrayValue::new(vec![])),
                    method,
                    &[],
                    args,
                    span,
                ),
                ControlSignal::Fault(_)
            ));
        }
    });
}

#[test]
fn array_and_stack_push_pop_reuse_unique_backing_and_allocate_shared_output_once() {
    with_collection_eval(|eval, expr, span| {
        for stack in [false, true] {
            for shared in [false, true] {
                for count in [1000, 2000, 4000] {
                    let mut buffer = Vec::with_capacity(count + 1);
                    buffer.extend((0..count).map(|_| InterpValue::String("x".repeat(1024).into())));
                    let values = ArrayValue::new(buffer);
                    let pointer = values.borrow().as_ptr();
                    let alias = shared.then(|| values.clone());
                    let receiver = if stack {
                        InterpValue::Stack(values)
                    } else {
                        InterpValue::Array(values)
                    };
                    let args = vec![InterpValue::String("last".into())];
                    let (result, cost) = measure(|| {
                        eval.eval_local_method_with_values(expr, receiver, "push", &[], args, span)
                    });
                    assert_eq!(
                        cost.count,
                        if shared { 2 } else { 0 },
                        "push stack={stack} shared={shared} n={count}: {cost:?}"
                    );
                    let ControlSignal::Value(receiver) = result else {
                        panic!("push result")
                    };
                    let values = match &receiver {
                        InterpValue::Array(v) | InterpValue::Stack(v) => v,
                        _ => panic!("receiver kind"),
                    };
                    assert_eq!(values.borrow().len(), count + 1);
                    if !shared {
                        assert_eq!(values.borrow().as_ptr(), pointer);
                    }
                    if let Some(alias) = &alias {
                        assert_eq!(alias.borrow().len(), count);
                        assert!(
                            cost.bytes <= (count + 1) * std::mem::size_of::<InterpValue>() + 128,
                            "duplicate backing/payload copy: {cost:?}"
                        );
                        let InterpValue::String(old) = &alias.borrow()[0] else {
                            panic!("string")
                        };
                        let InterpValue::String(new) = &values.borrow()[0] else {
                            panic!("string")
                        };
                        assert_eq!(old.as_ptr(), new.as_ptr());
                    }
                    eprintln!("push stack={stack} shared={shared} n={count}: {cost:?}");
                    let (popped, cost) = measure(|| {
                        eval.eval_local_method_with_values(expr, receiver, "pop", &[], vec![], span)
                    });
                    assert_eq!(
                        cost.count, 3,
                        "only pop tuple/Some storage, not a new Array backing: {cost:?}"
                    );
                    let ControlSignal::Value(InterpValue::Tuple(fields)) = popped else {
                        panic!("pop result")
                    };
                    let values = match &fields[0] {
                        InterpValue::Array(v) | InterpValue::Stack(v) => v,
                        _ => panic!("receiver kind"),
                    };
                    assert_eq!(values.borrow().len(), count);
                    assert_eq!(
                        fields[1],
                        InterpValue::OptionSome(InterpValue::String("last".into()).into())
                    );
                }
            }
        }
    });
}
