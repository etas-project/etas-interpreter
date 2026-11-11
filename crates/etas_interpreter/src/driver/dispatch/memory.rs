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

pub(in crate::driver) enum MemoryDispatchStart<'host> {
    Complete(Option<ControlSignal>),
    Pending(Box<PendingMemoryDispatch<'host>>),
}

pub(in crate::driver) struct PendingMemoryDispatch<'host> {
    memory: PendingMemory,
    key: etas_host::replay::ReplayFingerprint,
    io: MemoryIo<'host>,
}

enum MemoryIo<'host> {
    Read(
        super::host_dispatch::PendingHostOperation<
            MemoryResponse,
            crate::host::HostFuture<'host, Result<MemoryResponse, HostError>>,
        >,
    ),
    Write(super::memory_write::PendingMemoryWrite<'host>),
}

enum MemoryCompletion {
    Read(super::host_dispatch::HostCompletion<MemoryResponse>),
    Write(super::memory_write::CompletedMemoryWrite),
}

pub(in crate::driver) struct CompletedMemoryDispatch {
    memory: PendingMemory,
    key: etas_host::replay::ReplayFingerprint,
    completion: MemoryCompletion,
}

impl PendingMemoryDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        match &self.io {
            MemoryIo::Read(pending) => pending.operation_id(),
            MemoryIo::Write(pending) => pending.operation_id(),
        }
    }
    pub(in crate::driver) async fn poll(self) -> CompletedMemoryDispatch {
        let completion = match self.io {
            MemoryIo::Read(pending) => MemoryCompletion::Read(HostDispatch::poll(pending).await),
            MemoryIo::Write(pending) => MemoryCompletion::Write(pending.poll().await),
        };
        CompletedMemoryDispatch {
            memory: self.memory,
            key: self.key,
            completion,
        }
    }
}

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: PendingMemory,
    machine: &mut EvalMachine,
) -> Option<ControlSignal> {
    match begin(eval, host, memory, machine).await {
        MemoryDispatchStart::Complete(signal) => signal,
        MemoryDispatchStart::Pending(pending) => finish(eval, machine, pending.poll().await),
    }
}

pub(in crate::driver) async fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    memory: PendingMemory,
    machine: &mut EvalMachine,
) -> MemoryDispatchStart<'host> {
    let key = match prepare(eval, host, &memory, machine).await {
        Ok(key) => key,
        Err(signal) => return MemoryDispatchStart::Complete(signal),
    };
    let io = match memory.request.operation {
        MemoryOperation::Put { .. } | MemoryOperation::Delete { .. } => {
            match super::memory_write::begin(eval, host, memory.request.clone()) {
                Ok(pending) => MemoryIo::Write(pending),
                Err(failure) => {
                    return MemoryDispatchStart::Complete(write_failure(
                        eval, machine, memory, failure,
                    ));
                }
            }
        }
        MemoryOperation::Get { .. } => {
            use etas_host::HostTraceRequest;
            let request = memory.request.clone();
            match HostDispatch::begin(
                eval,
                request.id,
                HostRequestKind::Memory,
                request.trace_payload(),
                request.authority.clone(),
                request.trace.clone(),
                |operation| host.memory(operation, request),
            ) {
                Ok(pending) => MemoryIo::Read(pending),
                Err(error) => {
                    return MemoryDispatchStart::Complete(report_dispatched_error(
                        eval, machine, memory, error,
                    ));
                }
            }
        }
        _ => {
            let response =
                super::memory_pages::dispatch_read(eval, host, memory.request.clone()).await;
            return MemoryDispatchStart::Complete(finish_response(
                eval, machine, memory, key, response,
            ));
        }
    };
    MemoryDispatchStart::Pending(Box::new(PendingMemoryDispatch { memory, key, io }))
}

async fn prepare(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: &PendingMemory,
    machine: &mut EvalMachine,
) -> Result<etas_host::replay::ReplayFingerprint, Option<ControlSignal>> {
    let key = eval
        .memory_boundary_key(memory)
        .map_err(|fault| Some(ControlSignal::Fault(Box::new(fault))))?;
    if let Some(value) = eval
        .replayed_memory_result(memory, &key)
        .map_err(|fault| Some(ControlSignal::Fault(Box::new(fault))))?
    {
        if !validate_replayed_memory_version(eval, host, memory, &value).await {
            return Err(None);
        }
        return Err(Some(eval.resume_memory_signal(memory.clone(), value)));
    }
    if let Err(error) = memory.request.budget.check_time() {
        return Err(retry_or_report(
            eval,
            machine,
            memory.continuation.clone(),
            memory.span,
            format!("memory host boundary failed: {}", error.message),
        ));
    }
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
        return Err(None);
    }
    Ok(key)
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    completed: CompletedMemoryDispatch,
) -> Option<ControlSignal> {
    let CompletedMemoryDispatch {
        memory,
        key,
        completion,
    } = completed;
    let response = match completion {
        MemoryCompletion::Read(completion) => HostDispatch::finish(eval, completion),
        MemoryCompletion::Write(completion) => {
            let result = super::memory_write::finish(eval, completion);
            if let Some(signal) = eval.cancellation_signal(memory.span) {
                return Some(signal);
            }
            match result {
                Ok(result) => Ok(MemoryResponse {
                    id: memory.request.id,
                    result: Ok(result),
                }),
                Err(failure) => return write_failure(eval, machine, memory, failure),
            }
        }
    };
    finish_response(eval, machine, memory, key, response)
}

fn write_failure(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    memory: PendingMemory,
    failure: super::memory_write::WriteFailure,
) -> Option<ControlSignal> {
    match failure {
        super::memory_write::WriteFailure::NotCommitted(error) => retry_or_report(
            eval,
            machine,
            memory.continuation,
            memory.span,
            format!("memory write was not committed: {}", error.message),
        ),
        super::memory_write::WriteFailure::Terminal(error) => {
            Some(ControlSignal::runtime_fault(error.message, memory.span))
        }
    }
}

fn finish_response(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    memory: PendingMemory,
    key: etas_host::replay::ReplayFingerprint,
    response: Result<MemoryResponse, HostError>,
) -> Option<ControlSignal> {
    let request_id = memory.request.id;
    match response {
        Ok(response) => match response.result {
            Ok(etas_host::MemoryResult::Conflict(conflict)) => {
                Some(eval.memory_conflict_signal(memory, conflict))
            }
            Ok(result) => {
                eval.record_memory_result_versions(&memory.request, &result);
                match eval.memory_result_value(&memory, result) {
                    Ok(value) => {
                        if let Err(fault) = eval.record_completed_host_boundary(
                            crate::orchestration::BoundaryOccurrenceId::HostRequest(request_id),
                            "memory",
                            key,
                            value.clone(),
                            memory.span,
                        ) {
                            return Some(ControlSignal::Fault(Box::new(fault)));
                        }
                        Some(eval.resume_memory_signal(memory, value))
                    }
                    Err(fault) => Some(ControlSignal::Fault(Box::new(fault))),
                }
            }
            Err(error) => report_dispatched_error(eval, machine, memory, error),
        },
        Err(error) => report_dispatched_error(eval, machine, memory, error),
    }
}

fn report_dispatched_error(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    memory: PendingMemory,
    error: HostError,
) -> Option<ControlSignal> {
    retry_or_report(
        eval,
        machine,
        memory.continuation,
        memory.span,
        format!("memory host boundary failed: {}", error.message),
    )
}

fn policy_subject(request: &MemoryRequest) -> PolicySubject {
    let (operation, effect_action, expected_version) = match &request.operation {
        MemoryOperation::Get { .. } => ("get", "read", None),
        MemoryOperation::Put { condition, .. } => match condition {
            etas_host::WriteCondition::Missing => ("insert", "write", None),
            etas_host::WriteCondition::Exists => ("update", "write", None),
            etas_host::WriteCondition::Any => ("put", "write", None),
            etas_host::WriteCondition::Match(version) => ("put", "write", Some(version)),
        },
        MemoryOperation::Delete { condition, .. } => {
            ("delete", "write", condition.expected_version())
        }
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
            HostValue::String(version.as_token().to_owned()),
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
        (MemoryOperation::Scan { .. }, crate::eval::MemoryDecode::Page { .. }) => {
            validate_replayed_memory_page(eval, host, memory, replayed).await
        }
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

async fn validate_replayed_memory_page(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    memory: &PendingMemory,
    replayed: &InterpValue,
) -> bool {
    if !check_replay_budget(eval, memory) {
        return false;
    }
    let result = match dispatch_memory_request(eval, host, memory.request.clone()).await {
        Ok(response) => response.result,
        Err(error) => Err(error),
    };
    let message = match result {
        Ok(result) => match eval.memory_result_value(memory, result) {
            Ok(actual) if &actual == replayed => return true,
            Ok(_) => "checkpoint memory page no longer matches the recorded page".to_owned(),
            Err(fault) => {
                eval.diagnostics.push(fault.into_diagnostic());
                return false;
            }
        },
        Err(error) => format!(
            "checkpoint memory page validation failed: {}",
            error.message
        ),
    };
    eval.diagnostics.push(Diagnostic::analysis(
        AnalysisDiagnosticCode::UnhandledRuntimeError,
        memory.span,
        message,
    ));
    false
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
        Ok(MemoryResult::Value { version, .. }) => Some(version.as_token().to_owned()),
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
                    version.as_token().to_owned()
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
    let response =
        match super::memory_pages::dispatch_read(eval, host, memory.request.clone()).await {
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
                entry.version.as_token().to_owned(),
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

pub(super) async fn dispatch_memory_request(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    request: MemoryRequest,
) -> Result<MemoryResponse, HostError> {
    use etas_host::HostTraceRequest;

    let request_id = request.id;
    let trace_payload = request.trace_payload();
    let authority = request.authority.clone();
    let trace = request.trace.clone();
    HostDispatch::execute(
        eval,
        request_id,
        HostRequestKind::Memory,
        trace_payload,
        authority,
        trace,
        |operation| host.memory(operation, request),
    )
    .await
}
