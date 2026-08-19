use super::*;

pub fn run_report_json(
    command: &'static str,
    sources: &[PathBuf],
    flow: &str,
    result: &RunResult,
) -> Result<Value, InterpreterCodecError> {
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
        "value": result.value.as_ref().map(value_json),
        "events": result.events.iter().map(event_json).collect::<Vec<_>>(),
        "checkpoints": checkpoints,
        "diagnostics": result.diagnostics.iter().map(diagnostic_summary_json).collect::<Vec<_>>(),
    }))
}
