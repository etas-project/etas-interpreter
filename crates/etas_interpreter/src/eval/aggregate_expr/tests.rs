use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

#[test]
fn literal_suspension_does_not_copy_checked_descriptors() {
    for shape in ["array", "record", "map"] {
        for count in [1000, 2000, 4000] {
            let elements = (0..count)
                .map(|i| match shape {
                    "record" => format!("field_{i} = {i}"),
                    "map" => format!("{i} => {i}"),
                    _ => i.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",");
            let literal = if shape == "array" {
                format!("[{elements}]")
            } else {
                format!("{{{elements}}}")
            };
            let checked = crate::testing::project::checked_project(&format!(
                "module app.main; flow main() -> unit {{ let value = {literal}; return; }}"
            ));
            let plan = crate::Interpreter
                .plan(&checked, crate::api::PlanOptions)
                .plan
                .unwrap();
            let expr = checked
                .hir
                .exprs
                .iter()
                .find_map(|(id, data)| {
                    matches!(
                        data,
                        HirExpr::Array { .. } | HirExpr::Record(_) | HirExpr::Map { .. }
                    )
                    .then_some(id)
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
            let (signal, cost) = measure(|| eval.eval_expr_frame(expr, &mut frame));
            assert!(is_pending_host_boundary_signal(&signal));
            assert!(cost.count <= 6, "{shape} n={count}: {cost:?}");
            let result_slot = match shape {
                "record" => std::mem::size_of::<(String, InterpValue)>(),
                "map" => std::mem::size_of::<(InterpValue, InterpValue)>(),
                _ => std::mem::size_of::<InterpValue>(),
            };
            assert!(
                cost.bytes < count * result_slot + 4096,
                "only result slots and constant continuation storage: {shape} n={count}: {cost:?}"
            );
            eprintln!("literal suspension {shape} n={count}: {cost:?}");
            let ControlSignal::Expr(pending) = signal else {
                panic!("pending element");
            };
            let (_, snapshot_cost) = measure(|| {
                crate::orchestration::ContinuationSnapshot::capture(&pending.continuation).unwrap()
            });
            assert!(
                snapshot_cost.count <= 2 && snapshot_cost.bytes < 2048,
                "snapshot must not copy unevaluated descriptors: {shape} n={count}: {snapshot_cost:?}"
            );
            eprintln!("literal snapshot {shape} n={count}: {snapshot_cost:?}");
        }
    }
}
