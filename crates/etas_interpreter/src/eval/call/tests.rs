use super::*;
use crate::{api::RunOptions, testing::allocation::measure};

#[test]
fn argument_suspension_moves_descriptors_and_partial_results_without_copying() {
    for count in [1000, 2000, 4000] {
        let params = (0..count)
            .map(|i| format!("p{i}: i32"))
            .collect::<Vec<_>>()
            .join(",");
        let arguments = (0..count)
            .map(|i| format!("p{i} = {i}"))
            .collect::<Vec<_>>()
            .join(",");
        let checked = crate::testing::project::checked_project(&format!(
            "module app.main; flow consume({params}) -> unit {{ return; }} flow main() -> unit {{ consume({arguments}); }}"
        ));
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let (callee, args, span) = checked
            .hir
            .exprs
            .iter()
            .find_map(|(_, expr)| match expr {
                HirExpr::Call {
                    callee, args, span, ..
                } => Some((*callee, args, *span)),
                _ => None,
            })
            .unwrap();
        let HirExpr::Path(path) = &checked.hir.exprs[callee] else {
            panic!("callee")
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            panic!("resolution")
        };
        let SymbolDef::Item { item } = checked.symbols.get(symbol).unwrap().def else {
            panic!("flow")
        };
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
        let args = args.clone();
        let descriptor_pointer = args.as_ptr();
        let mut evaluated = Vec::with_capacity(count);
        evaluated.resize(count / 2, InterpValue::i32(0));
        let results_pointer = evaluated.as_ptr();
        let (signal, cost) = measure(|| {
            eval.resume_call_args(
                CallTarget::FlowItem(item),
                args,
                count / 2,
                evaluated,
                span,
                &mut frame,
            )
        });
        let ControlSignal::Expr(pending) = signal else {
            panic!("pending argument")
        };
        let continuation = call_args(&pending.continuation).unwrap();
        let Continuation::CallArgs {
            args,
            evaluated_args,
            next_arg_index,
            ..
        } = continuation
        else {
            unreachable!()
        };
        assert_eq!(args.as_ptr(), descriptor_pointer);
        assert_eq!(evaluated_args.as_ptr(), results_pointer);
        assert_eq!(*next_arg_index, count / 2 + 1);
        assert!(
            cost.count <= 2 && cost.bytes < 1024,
            "{count} parameters: {cost:?}"
        );
        eprintln!("argument suspension n={count}: {cost:?}");
    }
}

fn call_args(value: &Continuation) -> Option<&Continuation> {
    match value {
        Continuation::CallArgs { .. } => Some(value),
        Continuation::Chain { inner, outer } => call_args(inner).or_else(|| call_args(outer)),
        _ => None,
    }
}
