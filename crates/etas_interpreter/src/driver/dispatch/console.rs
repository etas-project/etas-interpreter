use etas_host::{
    HostRequestKind, HostTraceRequest, HostValue, PolicySubject,
    console::{ConsoleOperation, ConsoleRequest},
};

use crate::{
    control::{ControlSignal, PendingConsole},
    eval::EvalContext,
    host::HostServices,
};

use super::{host_dispatch::HostDispatch, policy::evaluate_before_boundary};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    console: PendingConsole,
) -> Option<ControlSignal> {
    if let Some(value) = eval.replayed_console_result(&console) {
        return Some(eval.resume_console_signal(console, value));
    }
    if let Err(error) = console.request.budget.check_time() {
        return Some(eval.console_host_error_signal(console, error));
    }
    let key = eval.console_boundary_key(&console);
    let request_id = console.request.id;
    let trace_subject = policy_subject(&console.request);
    if !evaluate_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        trace_subject.clone(),
        console.span,
        "console",
    )
    .await
    {
        return None;
    }
    match HostDispatch::execute(
        eval,
        request_id,
        HostRequestKind::Console,
        console.request.trace_payload(),
        console.request.authority.clone(),
        console.request.trace.clone(),
        host.console(console.request.clone()),
    )
    .await
    {
        Ok(response) => match eval.console_result_value(&console, response.result) {
            Ok(value) => {
                eval.record_completed_host_boundary(
                    crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                    "console",
                    key,
                    value.clone(),
                );
                Some(eval.resume_console_signal(console, value))
            }
            Err(fault) => Some(ControlSignal::Fault(Box::new(fault))),
        },
        Err(error) => Some(eval.console_host_error_signal(console, error)),
    }
}

fn policy_subject(request: &ConsoleRequest) -> PolicySubject {
    let (operation, effect_action, attributes) = match &request.operation {
        ConsoleOperation::ReadAllStdin => ("read_all_stdin", "stdin_read_all", Vec::new()),
        ConsoleOperation::ReadLineStdin => ("read_line_stdin", "stdin_read_line", Vec::new()),
        ConsoleOperation::WriteStdout { text, newline } => (
            "write_stdout",
            "stdout_write",
            vec![
                ("text_len".to_owned(), HostValue::UInt(text.len() as u128)),
                ("newline".to_owned(), HostValue::Bool(*newline)),
            ],
        ),
        ConsoleOperation::WriteStderr { text, newline } => (
            "write_stderr",
            "stderr_write",
            vec![
                ("text_len".to_owned(), HostValue::UInt(text.len() as u128)),
                ("newline".to_owned(), HostValue::Bool(*newline)),
            ],
        ),
    };
    let mut attributes = attributes;
    attributes.splice(
        0..0,
        [
            (
                "operation".to_owned(),
                HostValue::String(operation.to_owned()),
            ),
            (
                "action_kind".to_owned(),
                HostValue::String("console".to_owned()),
            ),
            (
                "qualified_action".to_owned(),
                HostValue::String(format!("Console.{effect_action}")),
            ),
            ("resource".to_owned(), HostValue::String("stdio".to_owned())),
        ],
    );
    PolicySubject {
        kind: "console".to_owned(),
        attributes,
    }
}
