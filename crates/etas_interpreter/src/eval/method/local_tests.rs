use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

#[test]
fn collection_kernels_move_owned_arguments_and_preserve_live_aliases() {
    let checked = crate::testing::project::checked_project(
        "module app.main; flow main() -> Array<string> { return [\"start\"].push(\"end\"); }",
    );
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
    for count in [1000, 2000, 4000] {
        let tail = crate::value::ListValue::new(
            (0..count)
                .map(|_| InterpValue::String("tail".repeat(128).into()))
                .collect(),
        );
        let receiver = InterpValue::List(tail.clone());
        let args = vec![InterpValue::String("head".repeat(128).into())];
        let (pushed, cost) =
            measure(|| eval.eval_local_method_with_values(expr, receiver, "push", &[], args, span));
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
        let (result, allocations) =
            measure(|| eval.eval_local_method_with_values(expr, left, "extend", &[], args, span));
        assert_eq!(
            allocations.count, 1,
            "only the resulting Rc, n={count}: {allocations:?}"
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
        drop(values);

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
            allocations.count, 3,
            "COW vector, growth and backing; text is shared"
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
}
