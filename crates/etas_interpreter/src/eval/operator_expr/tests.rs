use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

#[test]
fn array_operator_and_extend_reuse_unique_left_without_copying_shared_right() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> Array<string> { return [\"a\"] + [\"b\"].extend([\"c\"]); }",
    );
    let plan = crate::Interpreter
        .plan(&checked, crate::api::PlanOptions)
        .plan
        .unwrap();
    let (expr, span) = checked
        .hir
        .exprs
        .iter()
        .find_map(|(id, value)| match value {
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
    for method in [false, true] {
        for count in [1000, 2000, 4000] {
            let mut left = Vec::with_capacity(2 * count);
            left.extend((0..count).map(|_| InterpValue::String("left".repeat(256).into())));
            let pointer = left.as_ptr();
            let left = ArrayValue::new(left);
            let right = ArrayValue::new(
                (0..count)
                    .map(|_| InterpValue::String("right".repeat(256).into()))
                    .collect(),
            );
            let alias = right.clone();
            let args = vec![InterpValue::Array(right)];
            let (result, cost) = measure(|| {
                if method {
                    match eval.eval_local_method_with_values(
                        expr,
                        InterpValue::Array(left),
                        "extend",
                        &[],
                        args,
                        span,
                    ) {
                        ControlSignal::Value(value) => value,
                        _ => panic!("extend failed"),
                    }
                } else {
                    eval.eval_add_value(
                        InterpValue::Array(left),
                        args.into_iter().next().unwrap(),
                        span,
                    )
                    .unwrap()
                }
            });
            eprintln!("array concat method={method} n={count}: {cost:?}");
            assert_eq!(cost.count, 1, "only output owner should allocate: {cost:?}");
            assert!(cost.bytes < 128, "temporary input/output copy: {cost:?}");
            let InterpValue::Array(result) = result else {
                panic!("array")
            };
            let result = result.borrow();
            assert_eq!(result.as_ptr(), pointer);
            assert_eq!(result.len(), count * 2);
            for (a, b) in result[count..].iter().zip(alias.borrow().iter()) {
                let (InterpValue::String(a), InterpValue::String(b)) = (a, b) else {
                    panic!("text")
                };
                assert_eq!(a.as_ptr(), b.as_ptr());
            }
            assert_eq!(alias.borrow().len(), count);
        }
    }
}
