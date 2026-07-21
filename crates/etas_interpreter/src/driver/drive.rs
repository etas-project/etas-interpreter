use etas_core::{AnalysisDiagnosticCode, Diagnostic};

use crate::{
    control::ControlSignal,
    eval::{
        EvalContext,
        machine::{EvalMachine, MachinePoll, PendingBoundary},
    },
    host::HostServices,
    value::InterpValue,
};

use super::dispatch;

pub async fn execute_entry(
    eval: &mut EvalContext<'_>,
    signal: ControlSignal,
    host: &dyn HostServices,
) -> Option<InterpValue> {
    drive_eval_signal(eval, signal, host, EvalMachine::new()).await
}

pub async fn execute_entry_from_snapshot(
    eval: &mut EvalContext<'_>,
    signal: ControlSignal,
    host: &dyn HostServices,
    snapshot: &crate::orchestration::MachineSnapshot,
) -> Option<InterpValue> {
    let machine = match EvalMachine::from_snapshot(snapshot, eval.checked, eval.plan.slots.clone())
    {
        Ok(machine) => machine,
        Err(message) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::MissingCheckedFact,
                crate::diagnostics::item_span(eval.checked, eval.entry_item),
                format!("checkpoint machine stack is invalid: {message}"),
            ));
            return None;
        }
    };
    drive_eval_signal(eval, signal, host, machine).await
}

pub(super) async fn drive_eval_signal(
    eval: &mut EvalContext<'_>,
    mut signal: ControlSignal,
    host: &dyn HostServices,
    mut machine: EvalMachine,
) -> Option<InterpValue> {
    let mut machine_has_input = false;
    loop {
        if !machine_has_input {
            machine.resume(std::mem::replace(
                &mut signal,
                ControlSignal::Value(InterpValue::Unit),
            ));
        }
        machine_has_input = false;
        let boundary = match machine.run_until_yield(eval) {
            MachinePoll::Complete(value) => return Some(*value),
            MachinePoll::Yield(boundary) => boundary,
            MachinePoll::Fault(fault) => {
                eval.diagnostics.push(fault.into_diagnostic());
                return None;
            }
        };
        signal = match boundary {
            PendingBoundary::Model(model) => {
                if !dispatch::model::dispatch(eval, host, &model, &mut machine).await {
                    return None;
                }
                machine_has_input = true;
                continue;
            }
            PendingBoundary::Tool(tool) => {
                if !dispatch::tool::dispatch(eval, host, *tool, &mut machine).await {
                    return None;
                }
                machine_has_input = true;
                continue;
            }
            PendingBoundary::Perform(perform) => {
                dispatch::approval::dispatch(eval, host, *perform, &mut machine).await?
            }
            PendingBoundary::Memory(memory) => {
                dispatch::memory::dispatch(eval, host, *memory, &mut machine).await?
            }
            PendingBoundary::Session(session) => {
                dispatch::session::dispatch(eval, host, *session).await
            }
            PendingBoundary::Console(console) => {
                dispatch::console::dispatch(eval, host, *console).await?
            }
            PendingBoundary::Command(command) => {
                dispatch::command::dispatch(eval, host, *command, &mut machine).await?
            }
            PendingBoundary::Host(boundary) => {
                dispatch::host::dispatch(eval, host, *boundary, &mut machine).await?
            }
        };
    }
}
