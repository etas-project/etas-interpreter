use super::*;
use crate::{
    api::RunOptions,
    testing::{allocation::measure, project::checked_project},
};

#[test]
fn flow_entry_allocation_does_not_scale_with_unrelated_project_locals() {
    for count in [1000, 2000, 4000] {
        let params = (0..count)
            .map(|n| format!("unused_{n}: i32"))
            .collect::<Vec<_>>()
            .join(",");
        let checked = checked_project(&format!(
            "module app.main; flow unused({params}) -> unit {{ return; }} flow main(seed: i32) -> i32 {{ let local = seed; return local; }}"
        ));
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
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
        let item = checked.entry.unwrap();
        let HirItem::Flow(flow) = &checked.hir.items[item] else {
            panic!("flow")
        };
        assert_eq!(plan.slots.slot_count(), count + 2);
        assert_eq!(plan.frames.get(flow.scope).unwrap().slot_count(), 2);
        for _ in 0..2 {
            let (signal, cost) = measure(|| eval.execute_flow(item, flow, &[InterpValue::i32(42)]));
            let ControlSignal::Block(pending) = signal else {
                panic!("pending flow body")
            };
            assert_eq!(
                pending.frame.get(flow.params[0]),
                Some(InterpValue::i32(42))
            );
            eprintln!("flow entry with {count} unrelated locals: {cost:?}");
            assert!(
                cost.bytes < 4096,
                "flow allocated project-wide slots: {cost:?}"
            );
            assert!(cost.count <= 8, "{cost:?}");
        }
    }
}
