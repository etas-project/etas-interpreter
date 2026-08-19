use std::collections::BTreeMap;

use etas_core::{AnalysisDiagnosticCode, Diagnostic};
use etas_host::{
    HostError, HostRequestKind, HostValue, MemoryOperation, MemoryRequest, MemoryResponse,
    MemoryResult, PolicySubject,
};

use crate::{
    control::{ControlSignal, PendingMemory},
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
    value::InterpValue,
};

use super::{
    error::retry_or_report, host_dispatch::HostDispatch, policy::evaluate_before_boundary,
};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: PendingMemory,
    machine: &mut EvalMachine,
) -> Option<ControlSignal> {
    if let Some(value) = eval.replayed_memory_result(&memory) {
        if !validate_replayed_memory_version(eval, host, &memory, &value).await {
            return None;
        }
        return Some(eval.resume_memory_signal(memory, value));
    }
    if let Err(error) = memory.request.budget.check_time() {
        return retry_or_report(
            eval,
            machine,
            memory.continuation,
            memory.span,
            format!("memory host boundary failed: {}", error.message),
        );
    }
    let key = eval.memory_boundary_key(&memory);
    if !evaluate_before_boundary(
        eval,
        host,
        eval.boundary_policy_ref(),
        policy_subject(&memory.request),
        memory.span,
        "memory",
    )
    .await
    {
        return None;
    }
    match dispatch_memory_request(eval, host, memory.request.clone()).await {
        Ok(response) => match response.result {
            Ok(etas_host::MemoryResult::Conflict(conflict)) => {
                Some(eval.memory_conflict_signal(memory, conflict))
            }
            Ok(result) => {
                eval.record_memory_result_versions(&memory.request, &result);
                match eval.memory_result_value(&memory, result) {
                    Ok(value) => {
                        eval.record_completed_host_boundary("memory", key, value.clone());
                        Some(eval.resume_memory_signal(memory, value))
                    }
                    Err(fault) => Some(ControlSignal::Fault(Box::new(fault))),
                }
            }
            Err(error) => retry_or_report(
                eval,
                machine,
                memory.continuation,
                memory.span,
                format!("memory host boundary failed: {}", error.message),
            ),
        },
        Err(error) => retry_or_report(
            eval,
            machine,
            memory.continuation,
            memory.span,
            format!("memory host boundary failed: {}", error.message),
        ),
    }
}

fn policy_subject(request: &MemoryRequest) -> PolicySubject {
    let (operation, effect_action, expected_version) = match &request.operation {
        MemoryOperation::Get { .. } => ("get", "read", None),
        MemoryOperation::Put { expected, mode, .. } => match mode {
            etas_host::MemoryWriteMode::Insert => ("insert", "write", expected.as_ref()),
            etas_host::MemoryWriteMode::Update => ("update", "write", expected.as_ref()),
            etas_host::MemoryWriteMode::Upsert => ("upsert", "write", expected.as_ref()),
            etas_host::MemoryWriteMode::Put => ("put", "write", expected.as_ref()),
        },
        MemoryOperation::Delete { expected, .. } => ("delete", "write", expected.as_ref()),
        MemoryOperation::Scan { .. } => ("scan", "read", None),
        MemoryOperation::Query { .. } => ("query", "read", None),
        MemoryOperation::VectorSearch { .. } => ("vector_search", "read", None),
    };
    let resource = format!(
        "{}:{}",
        request.store.region.stable_id,
        request.store.path.join(".")
    );
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("memory".to_owned()),
        ),
        (
            "qualified_action".to_owned(),
            HostValue::String(format!("Memory.{effect_action}")),
        ),
        (
            "region".to_owned(),
            HostValue::String(request.store.region.stable_id.clone()),
        ),
        (
            "path".to_owned(),
            HostValue::String(request.store.path.join(".")),
        ),
        (
            "operation".to_owned(),
            HostValue::String(operation.to_owned()),
        ),
        ("resource".to_owned(), HostValue::String(resource)),
    ];
    if let Some(version) = expected_version {
        attributes.push((
            "expected_version".to_owned(),
            HostValue::String(version.opaque.clone()),
        ));
    }
    PolicySubject {
        kind: "memory".to_owned(),
        attributes,
    }
}

pub(in crate::driver) async fn validate_replayed_memory_version(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: &PendingMemory,
    replayed: &InterpValue,
) -> bool {
    match (&memory.request.operation, memory.decode) {
        (MemoryOperation::Get { .. }, _) => {
            validate_replayed_memory_get_version(eval, host, memory, replayed).await
        }
        (MemoryOperation::Scan { .. }, crate::eval::MemoryDecode::KeyList { .. }) => {
            validate_replayed_memory_scan_versions(eval, host, memory, replayed).await
        }
        (
            MemoryOperation::Scan { .. } | MemoryOperation::Query { .. },
            crate::eval::MemoryDecode::JsonEntries,
        ) => validate_replayed_memory_scan_versions(eval, host, memory, replayed).await,
        _ => true,
    }
}

pub(in crate::driver) async fn validate_replayed_memory_get_version(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: &PendingMemory,
    replayed: &InterpValue,
) -> bool {
    if !check_replay_budget(eval, memory) {
        return false;
    }
    let resources = match eval.memory_replay_resources(memory, replayed) {
        Ok(resources) => resources,
        Err(fault) => {
            eval.diagnostics.push(fault.into_diagnostic());
            return false;
        }
    };
    let Some(resource) = resources.into_iter().next() else {
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            memory.span,
            "checkpoint memory replay did not identify the requested resource",
        ));
        return false;
    };
    let Some(expected_version) = eval.recorded_memory_version(&resource).map(str::to_owned) else {
        if replayed_memory_get_absent(replayed) {
            return validate_replayed_memory_absence(eval, host, memory, &resource).await;
        }
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            memory.span,
            format!("checkpoint is missing a memory version for replayed resource `{resource}`"),
        ));
        return false;
    };
    let response = match dispatch_memory_request(eval, host, memory.request.clone()).await {
        Ok(response) => response,
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory version validation failed for `{resource}`: {}",
                    error.message
                ),
            ));
            return false;
        }
    };
    let actual_version = match response.result {
        Ok(MemoryResult::Value { version, .. }) => Some(version.opaque),
        Ok(MemoryResult::None) => None,
        Ok(other) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory version validation for `{resource}` returned unexpected result: {:?}",
                    other
                ),
            ));
            return false;
        }
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory version validation failed for `{resource}`: {}",
                    error.message
                ),
            ));
            return false;
        }
    };
    if actual_version.as_deref() == Some(expected_version.as_str()) {
        return true;
    }
    eval.diagnostics.push(Diagnostic::analysis(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        memory.span,
        format!(
            "checkpoint memory version mismatch for `{resource}`: expected {}, actual {}",
            expected_version,
            actual_version.as_deref().unwrap_or("<none>")
        ),
    ));
    false
}

pub(in crate::driver) fn replayed_memory_get_absent(replayed: &InterpValue) -> bool {
    matches!(replayed, InterpValue::OptionNone | InterpValue::Bool(false))
}

pub(in crate::driver) async fn validate_replayed_memory_absence(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: &PendingMemory,
    resource: &str,
) -> bool {
    if !check_replay_budget(eval, memory) {
        return false;
    }
    let response = match dispatch_memory_request(eval, host, memory.request.clone()).await {
        Ok(response) => response,
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory absence validation failed for `{resource}`: {}",
                    error.message
                ),
            ));
            return false;
        }
    };
    match response.result {
        Ok(MemoryResult::None) => true,
        Ok(MemoryResult::Value { version, .. }) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "checkpoint memory absence replay mismatch for `{resource}`: expected <none>, actual {}",
                    version.opaque
                ),
            ));
            false
        }
        Ok(other) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory absence validation for `{resource}` returned unexpected result: {:?}",
                    other
                ),
            ));
            false
        }
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "memory absence validation failed for `{resource}`: {}",
                    error.message
                ),
            ));
            false
        }
    }
}

pub(in crate::driver) async fn validate_replayed_memory_scan_versions(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: &PendingMemory,
    replayed: &InterpValue,
) -> bool {
    if !check_replay_budget(eval, memory) {
        return false;
    }
    let resources = match eval.memory_replay_resources(memory, replayed) {
        Ok(resources) => resources,
        Err(fault) => {
            eval.diagnostics.push(fault.into_diagnostic());
            return false;
        }
    };
    let mut expected_versions = BTreeMap::new();
    for resource in resources {
        let Some(version) = eval.recorded_memory_version(&resource).map(str::to_owned) else {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "checkpoint is missing a memory version for replayed resource `{resource}`"
                ),
            ));
            return false;
        };
        expected_versions.insert(resource, version);
    }
    let response = match dispatch_memory_request(eval, host, memory.request.clone()).await {
        Ok(response) => response,
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!("memory scan version validation failed: {}", error.message),
            ));
            return false;
        }
    };
    let entries = match response.result {
        Ok(MemoryResult::Entries { entries, .. }) => entries,
        Ok(other) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!("memory scan version validation returned unexpected result: {other:?}"),
            ));
            return false;
        }
        Err(error) => {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!("memory scan version validation failed: {}", error.message),
            ));
            return false;
        }
    };
    let actual_versions = entries
        .iter()
        .map(|entry| {
            (
                eval.memory_resource_for_key(memory, &entry.key),
                entry.version.opaque.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let expected_resources = expected_versions.keys().cloned().collect::<Vec<_>>();
    let actual_resources = actual_versions.keys().cloned().collect::<Vec<_>>();
    if expected_resources != actual_resources {
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            memory.span,
            format!(
                "checkpoint memory scan replay mismatch: expected resources [{}], actual resources [{}]",
                expected_resources.join(", "),
                actual_resources.join(", ")
            ),
        ));
        return false;
    }
    for (resource, expected_version) in expected_versions {
        let Some(actual_version) = actual_versions.get(&resource) else {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!("checkpoint memory scan replay mismatch: missing resource `{resource}`"),
            ));
            return false;
        };
        if actual_version != &expected_version {
            eval.diagnostics.push(Diagnostic::analysis(
                AnalysisDiagnosticCode::UnhandledRuntimeError,
                memory.span,
                format!(
                    "checkpoint memory version mismatch for `{resource}`: expected {}, actual {}",
                    expected_version, actual_version
                ),
            ));
            return false;
        }
    }
    true
}

fn check_replay_budget(eval: &mut EvalContext<'_>, memory: &PendingMemory) -> bool {
    if let Err(error) = memory.request.budget.check_time() {
        eval.diagnostics.push(Diagnostic::analysis(
            AnalysisDiagnosticCode::UnhandledRuntimeError,
            memory.span,
            format!(
                "memory replay validation exhausted the execution budget: {}",
                error.message
            ),
        ));
        return false;
    }
    true
}

async fn dispatch_memory_request(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    request: MemoryRequest,
) -> Result<MemoryResponse, HostError> {
    let request_id = request.id;
    let authority = request.authority.clone();
    let trace = request.trace.clone();
    HostDispatch::execute(
        eval,
        request_id,
        HostRequestKind::Memory,
        authority,
        trace,
        host.memory(request),
    )
    .await
}
