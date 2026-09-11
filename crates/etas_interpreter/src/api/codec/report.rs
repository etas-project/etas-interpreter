use super::*;

pub fn run_report_json(
    command: &'static str,
    sources: &[PathBuf],
    flow: &str,
    result: &RunResult,
) -> Result<Value, InterpreterCodecError> {
    let report = &result.termination;
    let checkpoints = result
        .checkpoints
        .iter()
        .map(checkpoint_json)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "schema": "etas.cli.interpreter-report.v1",
        "command": command,
        "sources": sources,
        "flow": flow,
        "outcome": match &result.outcome {
            crate::api::RunOutcome::Completed(_) => json!({"kind": "completed"}),
            crate::api::RunOutcome::Failed(failure) => json!({"kind": "failed", "failure": failure_json(failure)}),
            crate::api::RunOutcome::Cancelled(cause) => json!({
                "kind": "cancelled", "origin_scope": cause.origin().as_u64(),
                "reason": format!("{:?}", cause.reason()),
            }),
        },
        "termination": {
            "scope": report.scope().as_u64(),
            "local_work_settled": true,
            "operations": report.operations().iter().map(|operation| json!({
                "id": operation.id().as_u64(),
                "scope": operation.scope().as_u64(),
                "parent_operation": operation.parent().map(|id| id.as_u64()),
                "completed_units": operation.completed_units(),
                "request_id": operation.request().map(|id| id.0),
                "dispatched": operation.dispatched(),
                "external_outcome": match operation.outcome() {
                    Some(etas_host::execution::ExternalOutcome::NotDispatched) => json!({"kind": "not_dispatched"}),
                    Some(etas_host::execution::ExternalOutcome::Confirmed) => json!({"kind": "confirmed"}),
                    Some(etas_host::execution::ExternalOutcome::Partial { completed_units }) => json!({"kind": "partial", "completed_units": completed_units}),
                    Some(etas_host::execution::ExternalOutcome::Failed(error)) => json!({"kind": "failed", "code": format!("{:?}", error.code)}),
                    Some(etas_host::execution::ExternalOutcome::Unknown) => json!({"kind": "unknown"}),
                    Some(etas_host::execution::ExternalOutcome::StorageWrite(evidence)) => json!({"kind": "storage_write", "evidence": evidence}),
                    None => json!({"kind": "pending"}),
                },
                "cleanup_errors": operation.cleanup_errors().iter().map(|error| format!("{:?}", error.code)).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        },
        "value": result.value().map(value_json),
        "events": result.events.iter().map(event_json).collect::<Vec<_>>(),
        "checkpoints": checkpoints,
        "diagnostics": result.diagnostics.iter().map(diagnostic_summary_json).collect::<Vec<_>>(),
    }))
}

fn failure_json(failure: &crate::api::RunFailure) -> Value {
    use crate::api::RunFailure;
    match failure {
        RunFailure::PreparationRejected { origin } => {
            json!({"kind": "preparation_rejected", "origin": origin})
        }
        RunFailure::RestoreRejected { origin } => {
            json!({"kind": "restore_rejected", "origin": origin})
        }
        RunFailure::Language(diagnostic) => {
            json!({"kind": "language", "diagnostic": diagnostic_summary_json(diagnostic)})
        }
        RunFailure::ExecutionFault(fault) => {
            json!({"kind": "execution_fault", "diagnostic": diagnostic_summary_json(&fault.clone().into_diagnostic())})
        }
    }
}
