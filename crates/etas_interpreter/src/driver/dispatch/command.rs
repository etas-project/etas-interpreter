use etas_host::{CommandRequest, HostValue, PolicySubject};

use crate::{
    control::{ControlSignal, PendingCommand},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
};

use super::{error::retry_or_report, policy::evaluate_before_boundary};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    command: PendingCommand,
    machine: &mut EvalMachine,
) -> Option<ControlSignal> {
    if let Some(value) = eval.replayed_command_result(&command) {
        return Some(eval.resume_command_signal(command, value));
    }
    let key = eval.command_boundary_key(&command);
    let request_id = command.request.id;
    if !evaluate_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        policy_subject(&command.request),
        command.span,
        "command",
    )
    .await
    {
        return None;
    }
    eval.record_host_request_sent(request_id);
    match host.command(command.request.clone()).await {
        Ok(response) => {
            eval.record_host_response_received(response.id);
            match response.result {
                Ok(output) => {
                    let value = eval.command_result_value(&command, output)?;
                    eval.record_completed_host_boundary("command", key, value.clone());
                    Some(eval.resume_command_signal(command, value))
                }
                Err(error) => retry_or_report(
                    eval,
                    machine,
                    command.continuation,
                    command.span,
                    format!("command host boundary failed: {}", error.message),
                ),
            }
        }
        Err(error) => {
            eval.record_host_response_received(request_id);
            retry_or_report(
                eval,
                machine,
                command.continuation,
                command.span,
                format!("command host boundary failed: {}", error.message),
            )
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
