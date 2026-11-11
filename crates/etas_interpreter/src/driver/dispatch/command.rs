use etas_host::{
    CommandRequest, CommandResponse, HostRequestKind, HostTraceRequest, HostValue, PolicySubject,
};

use crate::{
    control::{ControlSignal, PendingCommand},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
};

use super::{
    error::retry_or_report,
    host_dispatch::{HostCompletion, HostDispatch, PendingHostOperation},
    policy::evaluate_before_boundary,
};

type CommandCall<'host> =
    crate::host::HostFuture<'host, Result<CommandResponse, etas_host::HostError>>;

pub(in crate::driver) enum CommandDispatchStart<'host> {
    Complete(Option<ControlSignal>),
    Pending(Box<PendingCommandDispatch<'host>>),
}

pub(in crate::driver) struct PendingCommandDispatch<'host> {
    command: PendingCommand,
    key: etas_host::replay::ReplayFingerprint,
    pending: PendingHostOperation<CommandResponse, CommandCall<'host>>,
}

pub(in crate::driver) struct CompletedCommandDispatch {
    command: PendingCommand,
    key: etas_host::replay::ReplayFingerprint,
    completion: HostCompletion<CommandResponse>,
}

pub(in crate::driver) async fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    mut command: PendingCommand,
    machine: &mut EvalMachine,
) -> CommandDispatchStart<'host> {
    let key = match eval.command_boundary_key(&command) {
        Ok(key) => key,
        Err(fault) => {
            return CommandDispatchStart::Complete(Some(ControlSignal::Fault(Box::new(fault))));
        }
    };
    match eval.completed_host_boundary_result(
        &crate::orchestration::BoundaryOccurrenceId::HostRequest(command.request.id),
        "command",
        &key,
        command.span,
    ) {
        Ok(Some(value)) => {
            return CommandDispatchStart::Complete(Some(
                eval.resume_command_signal(command, value),
            ));
        }
        Ok(None) => (),
        Err(fault) => {
            return CommandDispatchStart::Complete(Some(ControlSignal::Fault(Box::new(fault))));
        }
    }
    if let Err(error) = command.request.budget.check_time() {
        return CommandDispatchStart::Complete(retry_or_report(
            eval,
            machine,
            command.continuation,
            command.span,
            format!("command host boundary failed: {}", error.message),
        ));
    }
    let request_id = command.request.id;
    let trace_subject = policy_subject(&command.request);
    if !evaluate_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        trace_subject.clone(),
        command.span,
        "command",
    )
    .await
    {
        return CommandDispatchStart::Complete(None);
    }
    // Policy evaluation may record a scoped approval grant. Dispatch the
    // boundary with that current authority rather than the snapshot captured
    // when the pending command was first constructed.
    command.request.authority = eval.host_authority();
    match HostDispatch::begin(
        eval,
        request_id,
        HostRequestKind::Command,
        command.request.trace_payload(),
        command.request.authority.clone(),
        command.request.trace.clone(),
        |operation| host.command(operation, command.request.clone()),
    ) {
        Ok(pending) => CommandDispatchStart::Pending(Box::new(PendingCommandDispatch {
            command,
            key,
            pending,
        })),
        Err(error) => CommandDispatchStart::Complete(retry_or_report(
            eval,
            machine,
            command.continuation,
            command.span,
            format!("command host boundary failed: {}", error.message),
        )),
    }
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    completed: CompletedCommandDispatch,
) -> Option<ControlSignal> {
    let CompletedCommandDispatch {
        command,
        key,
        completion,
    } = completed;
    let request_id = command.request.id;
    match HostDispatch::finish(eval, completion) {
        Ok(response) => match response.result {
            Ok(output) => {
                let value = eval.command_result_value(&command, output)?;
                if let Err(fault) = eval.record_completed_host_boundary(
                    crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                    "command",
                    key,
                    value.clone(),
                    command.span,
                ) {
                    return Some(ControlSignal::Fault(Box::new(fault)));
                }
                Some(eval.resume_command_signal(command, value))
            }
            Err(error) => retry_or_report(
                eval,
                machine,
                command.continuation,
                command.span,
                format!("command host boundary failed: {}", error.message),
            ),
        },
        Err(error) => retry_or_report(
            eval,
            machine,
            command.continuation,
            command.span,
            format!("command host boundary failed: {}", error.message),
        ),
    }
}

impl PendingCommandDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.pending.operation_id()
    }

    pub(in crate::driver) async fn poll(self) -> CompletedCommandDispatch {
        CompletedCommandDispatch {
            command: self.command,
            key: self.key,
            completion: HostDispatch::poll(self.pending).await,
        }
    }
}

fn policy_subject(request: &CommandRequest) -> PolicySubject {
    let program = request.argv.first().cloned().unwrap_or_default();
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("command".to_owned()),
        ),
        (
            "qualified_action".to_owned(),
            HostValue::String("Command.run".to_owned()),
        ),
        ("operation".to_owned(), HostValue::String("run".to_owned())),
        ("program".to_owned(), HostValue::String(program.clone())),
        ("resource".to_owned(), HostValue::String(program)),
    ];
    attributes.push((
        "argc".to_owned(),
        HostValue::UInt(request.argv.len() as u128),
    ));
    if let Some(cwd) = &request.cwd {
        attributes.push(("cwd".to_owned(), HostValue::String(format!("{cwd:?}"))));
    }
    PolicySubject {
        kind: "command".to_owned(),
        attributes,
    }
}
