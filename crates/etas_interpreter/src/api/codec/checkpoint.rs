use super::*;
use crate::orchestration::BoundaryOccurrenceId;
use std::num::{NonZeroU32, NonZeroU64};

pub fn checkpoint_artifact_json(
    sources: &[PathBuf],
    flow: &str,
    checkpoint: &InterpreterCheckpoint,
) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "schema": crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA,
        "sources": sources,
        "flow": flow,
        "checkpoint": checkpoint_json(checkpoint)?,
    }))
}

pub fn checkpoint_id(checkpoint: &InterpreterCheckpoint) -> u32 {
    checkpoint.id.0
}

pub fn checkpoint_from_json_with_limits(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<InterpreterCheckpoint, InterpreterCodecError> {
    limits
        .validate()
        .map_err(|e| InterpreterCodecError::new(e.message))?;
    let schema = required_str(value, "schema")?;
    if schema != crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA {
        return Err(InterpreterCodecError::new(format!(
            "unsupported checkpoint artifact schema `{schema}`; expected `{}`",
            crate::orchestration::CHECKPOINT_ARTIFACT_SCHEMA
        )));
    }
    let checkpoint = value
        .get("checkpoint")
        .ok_or_else(|| InterpreterCodecError::new("checkpoint artifact is missing `checkpoint`"))?;
    let entry_item = HirItemId(required_u32(checkpoint, "entry_item")?);
    if checked.hir.items.get(entry_item).is_none() {
        return Err(InterpreterCodecError::new(format!(
            "checkpoint entry item {} does not exist in the checked HIR",
            entry_item.0
        )));
    }
    let compilation = compilation_identity_from_json(required_obj(checkpoint, "compilation")?)?;
    compilation
        .validate_for_project(checked, entry_item)
        .map_err(InterpreterCodecError::new)?;
    let host_state = checkpoint_host_state_from_json(required_obj(checkpoint, "host_state")?)?;
    host_state
        .budget
        .restore()
        .map_err(|error| InterpreterCodecError::new(error.to_string()))?;
    let machine = machine_from_json(limits, required_obj(checkpoint, "machine")?, checked)?;
    let checkpoint = InterpreterCheckpoint {
        storage: serde_json::from_value(required_obj(checkpoint, "storage")?.clone()).map_err(
            |error| {
                InterpreterCodecError::new(format!("invalid checkpoint storage state: {error}"))
            },
        )?,
        id: CheckpointId(required_u32(checkpoint, "id")?),
        label: required_optional_string(checkpoint, "label")?,
        compilation,
        entry_item,
        args: values_from_array(limits, checkpoint, "args")?,
        machine,
        handlers: HandlerSnapshot {
            handlers: required_array(checkpoint, "handlers")?
                .iter()
                .map(active_handler_from_json)
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        },
        retry_state: RetrySnapshot {
            attempts: required_array(checkpoint, "retry_state")?
                .iter()
                .map(|attempt| {
                    Ok(RetryAttemptRecord {
                        id: RetryAttemptId(required_u32(attempt, "id")?),
                        ordinal: required_u32(attempt, "ordinal")?,
                    })
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        },
        trace: {
            let trace = required_obj(checkpoint, "trace")?;
            TraceSnapshot {
                events_recorded: required_usize(trace, "events_recorded")?,
                next_message: required_u32(trace, "next_message")?,
                next_host_request: required_u32(trace, "next_host_request")?,
            }
        },
        execution_progress: execution_progress_from_json(required_obj(
            checkpoint,
            "execution_progress",
        )?)?,
        host_state,
        current_session: required_optional_string(checkpoint, "current_session")?,
        resource_versions: ResourceVersionSnapshot {
            versions: required_array(checkpoint, "resource_versions")?
                .iter()
                .map(resource_version_from_json)
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        },
        completed_host_boundaries: HostBoundaryLedger {
            completed: required_array(checkpoint, "completed_host_boundaries")?
                .iter()
                .map(|boundary| {
                    Ok(CompletedHostBoundary {
                        occurrence: boundary_occurrence_from_json(required_obj(
                            boundary,
                            "occurrence",
                        )?)?,
                        kind: required_str(boundary, "kind")?.to_owned(),
                        key: required_str(boundary, "key")?.to_owned(),
                        result: completed_host_boundary_result_from_json(
                            limits,
                            required_obj(boundary, "result")?,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        },
    };
    let slots = crate::plan::SlotLayoutTable::for_project(checked);
    let dispatch = crate::plan::IntrinsicDispatchTable::for_project(checked)
        .map_err(|errors| InterpreterCodecError::new(errors.join("; ")))?;
    crate::eval::machine::snapshot::SnapshotValidator::new(checked, &slots, &dispatch, limits)
        .validate_checkpoint(&checkpoint)
        .map_err(InterpreterCodecError::new)?;
    Ok(checkpoint)
}

pub fn sources_and_flow_from_checkpoint_json(
    value: &Value,
) -> Result<(Vec<PathBuf>, String), InterpreterCodecError> {
    let sources = value
        .get("sources")
        .and_then(Value::as_array)
        .ok_or_else(|| InterpreterCodecError::new("checkpoint artifact is missing `sources`"))?
        .iter()
        .map(|source| {
            source.as_str().map(PathBuf::from).ok_or_else(|| {
                InterpreterCodecError::new("checkpoint artifact contains a non-string source path")
            })
        })
        .collect::<Result<Vec<_>, InterpreterCodecError>>()?;
    if sources.is_empty() {
        return Err(InterpreterCodecError::new(
            "checkpoint artifact does not contain any source paths",
        ));
    }
    let flow = required_str(value, "flow")?.to_owned();
    Ok((sources, flow))
}

pub fn event_json(event: &WorkflowEvent) -> Value {
    match event {
        WorkflowEvent::StepStarted(id) => json!({ "kind": "step_started", "id": id.0 }),
        WorkflowEvent::StepCompleted(id) => json!({ "kind": "step_completed", "id": id.0 }),
        WorkflowEvent::HostTrace(event) => host_trace_event_json(event),
        WorkflowEvent::StorageWrite { request, evidence } => {
            json!({ "kind": "storage_write", "request": request.0, "evidence": evidence })
        }
        WorkflowEvent::CheckpointCreated(id) => json!({ "kind": "checkpoint_created", "id": id.0 }),
        WorkflowEvent::MessageCreated {
            id,
            from,
            to,
            session,
            role,
            created_at,
            payload,
            provenance,
        } => json!({
            "kind": "message_created",
            "id": id,
            "from": from,
            "to": to,
            "session": session,
            "role": role,
            "created_at": created_at,
            "payload": value_json(payload),
            "provenance": provenance.as_ref().map(provenance_json),
        }),
        WorkflowEvent::MessageSessionAttached {
            id,
            session,
            session_config,
        } => json!({
            "kind": "message_session_attached",
            "id": id,
            "session": session,
            "session_config": session_config_json(session_config),
        }),
        WorkflowEvent::MessageHandoff {
            id,
            from,
            to,
            session,
            target_item,
        } => json!({
            "kind": "message_handoff",
            "id": id,
            "from": from,
            "to": to,
            "session": session,
            "target_item": target_item,
        }),
        WorkflowEvent::AgentTracePlan { item, trace } => json!({
            "kind": "agent_trace_plan",
            "item": item,
            "trace": trace,
        }),
        WorkflowEvent::SessionResolved { session, created } => json!({
            "kind": "session_resolved",
            "session": session,
            "created": created,
        }),
        WorkflowEvent::SessionMessageAppended {
            session,
            message,
            deduplicated,
        } => json!({
            "kind": "session_message_appended",
            "session": session,
            "message": message,
            "deduplicated": deduplicated,
        }),
        WorkflowEvent::SessionHistoryLoaded {
            session,
            message_count,
            has_summary,
            cursor,
        } => json!({
            "kind": "session_history_loaded",
            "session": session,
            "message_count": message_count,
            "has_summary": has_summary,
            "cursor": cursor,
        }),
        WorkflowEvent::RetryAttemptStarted(id) => {
            json!({ "kind": "retry_attempt_started", "id": id.0 })
        }
        WorkflowEvent::RetryAttemptSucceeded(id) => {
            json!({ "kind": "retry_attempt_succeeded", "id": id.0 })
        }
        WorkflowEvent::RetryAttemptFailed(id) => {
            json!({ "kind": "retry_attempt_failed", "id": id.0 })
        }
        WorkflowEvent::RetryExhausted => json!({ "kind": "retry_exhausted" }),
        WorkflowEvent::ModelRepairAttempted {
            kind,
            attempt,
            reason,
        } => json!({
            "kind": "model_repair_attempted",
            "repair_kind": kind,
            "attempt": attempt,
            "reason": reason,
        }),
        WorkflowEvent::ModelRepairExhausted {
            kind,
            attempts,
            reason,
        } => json!({
            "kind": "model_repair_exhausted",
            "repair_kind": kind,
            "attempts": attempts,
            "reason": reason,
        }),
    }
}

fn host_trace_event_json(event: &etas_host::TraceEvent) -> Value {
    match event {
        etas_host::TraceEvent::HostRequestStarted {
            id,
            kind,
            metadata,
            authority,
            trace,
            started_at_unix_micros,
        } => json!({
            "kind": "host_request_started",
            "id": id.0,
            "request_kind": host_request_kind_name(*kind),
            "qualified_action": metadata.qualified_action,
            "subject_kind": metadata.subject_kind,
            "payload": metadata.fields.iter().map(|field| json!({
                "name": field.name,
                "sensitivity": host_trace_field_sensitivity_name(field.sensitivity),
                "value": field.value.as_ref().map(host_value_json),
            })).collect::<Vec<_>>(),
            "payload_digest": metadata.payload_digest,
            "started_at_unix_micros": started_at_unix_micros,
            "trace": {
                "trace_id": trace.trace_id.to_hex(),
                "parent_trace": trace.parent_trace.map(TraceId::to_hex),
                "parent_span": trace.parent_span.map(|span| span.0),
            },
            "authority": {
                "grant_count": authority.grants.len(),
                "approval_count": authority.approvals.len(),
                "active_trace_specs": authority.policy.active_trace_specs,
            },
        }),
        etas_host::TraceEvent::HostRequestFinished {
            command_isolation,
            id,
            outcome,
            finished_at_unix_micros,
            duration_micros,
        } => {
            let mut value = json!({
            "kind": "host_request_finished",
            "id": id.0,
            "outcome": host_outcome_json(outcome),
            "finished_at_unix_micros": finished_at_unix_micros,
            "duration_micros": duration_micros,
            });
            if let Some(report) = command_isolation {
                let guarantees = |value: etas_host::IsolationRequirements| {
                    json!({
                        "filesystem": value.filesystem, "network": value.network, "process": value.process,
                    })
                };
                value["command_isolation"] = json!({
                    "platform": report.platform(),
                    "backend": report.backend().map(|backend| backend.name()),
                    "requested": guarantees(report.requested()),
                    "active": guarantees(report.active()),
                });
            }
            value
        }
        etas_host::TraceEvent::ApprovalRequested {
            id,
            metadata,
            trace,
        } => json!({
            "kind": "approval_requested",
            "id": id.0,
            "qualified_action": metadata.qualified_action,
            "subject_kind": metadata.subject_kind,
            "payload": metadata.fields.iter().map(|field| json!({
                "name": field.name,
                "sensitivity": host_trace_field_sensitivity_name(field.sensitivity),
                "value": field.value.as_ref().map(host_value_json),
            })).collect::<Vec<_>>(),
            "payload_digest": metadata.payload_digest,
            "trace": {
                "trace_id": trace.trace_id.to_hex(),
                "parent_trace": trace.parent_trace.map(TraceId::to_hex),
                "parent_span": trace.parent_span.map(|span| span.0),
            },
        }),
    }
}

fn host_trace_field_sensitivity_name(
    sensitivity: etas_host::HostTraceFieldSensitivity,
) -> &'static str {
    match sensitivity {
        etas_host::HostTraceFieldSensitivity::Public => "public",
        etas_host::HostTraceFieldSensitivity::Sensitive => "sensitive",
        etas_host::HostTraceFieldSensitivity::Secret => "secret",
    }
}

fn host_outcome_json(outcome: &etas_host::HostOutcome) -> Value {
    match outcome {
        etas_host::HostOutcome::Succeeded => json!({ "kind": "succeeded" }),
        etas_host::HostOutcome::Failed(error) => json!({
            "kind": "failed",
            "code": error.code.as_str(),
            "message": error.message,
            "details": error.details.iter().map(|detail| json!({
                "key": detail.key,
                "value": detail.value,
            })).collect::<Vec<_>>(),
        }),
        etas_host::HostOutcome::Cancelled { reason } => {
            json!({ "kind": "cancelled", "reason": reason })
        }
    }
}

fn host_request_kind_name(kind: etas_host::HostRequestKind) -> &'static str {
    match kind {
        etas_host::HostRequestKind::Model => "model",
        etas_host::HostRequestKind::Tool => "tool",
        etas_host::HostRequestKind::Approval => "approval",
        etas_host::HostRequestKind::Memory => "memory",
        etas_host::HostRequestKind::Session => "session",
        etas_host::HostRequestKind::Console => "console",
        etas_host::HostRequestKind::Tcp => "tcp",
        etas_host::HostRequestKind::Stream => "stream",
        etas_host::HostRequestKind::Tls => "tls",
        etas_host::HostRequestKind::Filesystem => "filesystem",
        etas_host::HostRequestKind::Secret => "secret",
        etas_host::HostRequestKind::Browser => "browser",
        etas_host::HostRequestKind::Command => "command",
        etas_host::HostRequestKind::Policy => "policy",
    }
}

pub(super) fn checkpoint_json(
    checkpoint: &InterpreterCheckpoint,
) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "id": checkpoint.id.0,
        "label": checkpoint.label,
        "compilation": compilation_identity_json(&checkpoint.compilation),
        "entry_item": checkpoint.entry_item.0,
        "args": checkpoint.args.iter().map(value_json).collect::<Vec<_>>(),
        "machine": machine_json(&checkpoint.machine)?,
        "handlers": checkpoint.handlers.handlers.iter().map(|handler| {
            json!({
                "id": handler.id.0,
                "handled_actions": handler.handled_actions,
                "span": span_json(handler.span),
                "handlers": handler.handlers.iter().map(handler_arm_json).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
        "retry_state": checkpoint.retry_state.attempts.iter().map(|attempt| {
            json!({ "id": attempt.id.0, "ordinal": attempt.ordinal })
        }).collect::<Vec<_>>(),
        "trace": {
            "events_recorded": checkpoint.trace.events_recorded,
            "next_message": checkpoint.trace.next_message,
            "next_host_request": checkpoint.trace.next_host_request,
        },
        "execution_progress": execution_progress_json(checkpoint.execution_progress),
        "host_state": checkpoint_host_state_json(&checkpoint.host_state)?,
        "storage": checkpoint.storage,
        "current_session": checkpoint.current_session,
        "resource_versions": checkpoint.resource_versions.versions.iter().map(|version| {
            json!({ "resource": version.resource, "version": version.version })
        }).collect::<Vec<_>>(),
        "completed_host_boundaries": checkpoint.completed_host_boundaries.completed.iter().map(|boundary| {
            json!({
                "occurrence": boundary_occurrence_json(&boundary.occurrence),
                "kind": boundary.kind,
                "key": boundary.key,
                "result": completed_host_boundary_result_json(&boundary.result),
            })
        }).collect::<Vec<_>>(),
    }))
}

fn boundary_occurrence_json(occurrence: &BoundaryOccurrenceId) -> Value {
    match occurrence {
        BoundaryOccurrenceId::HostRequest(id) => json!({
            "kind": "host_request",
            "request_id": id.0,
        }),
        BoundaryOccurrenceId::SourceToolCall {
            model_request,
            call_id,
        } => json!({
            "kind": "source_tool_call",
            "model_request_id": model_request.0,
            "call_id": call_id,
        }),
    }
}

fn boundary_occurrence_from_json(
    value: &Value,
) -> Result<BoundaryOccurrenceId, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "host_request" => Ok(BoundaryOccurrenceId::HostRequest(etas_host::HostRequestId(
            required_u32(value, "request_id")?,
        ))),
        "source_tool_call" => Ok(BoundaryOccurrenceId::SourceToolCall {
            model_request: etas_host::HostRequestId(required_u32(value, "model_request_id")?),
            call_id: required_str(value, "call_id")?.to_owned(),
        }),
        other => Err(InterpreterCodecError::new(format!(
            "unknown completed host boundary occurrence kind `{other}`"
        ))),
    }
}

fn execution_progress_json(progress: ExecutionProgressSnapshot) -> Value {
    json!({
        "consumed_steps": progress.consumed_steps,
        "original_limits": {
            "max_call_depth": progress.original_limits.max_call_depth.get(),
            "max_steps": progress.original_limits.max_steps.map(NonZeroU64::get),
        },
    })
}

fn execution_progress_from_json(
    value: &Value,
) -> Result<ExecutionProgressSnapshot, InterpreterCodecError> {
    let limits = required_obj(value, "original_limits")?;
    let max_call_depth = NonZeroU32::new(required_u32(limits, "max_call_depth")?)
        .ok_or_else(|| InterpreterCodecError::new("checkpoint max_call_depth must be non-zero"))?;
    let max_steps = match limits.get("max_steps") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().and_then(NonZeroU64::new).ok_or_else(|| {
            InterpreterCodecError::new("checkpoint max_steps must be a non-zero u64")
        })?),
    };
    let original_limits = crate::api::ExecutionLimits::new(max_call_depth, max_steps)
        .map_err(InterpreterCodecError::new)?;
    Ok(ExecutionProgressSnapshot {
        consumed_steps: required_u64(value, "consumed_steps")?,
        original_limits,
    })
}

fn completed_host_boundary_result_json(result: &CompletedHostBoundaryResult) -> Value {
    match result {
        CompletedHostBoundaryResult::Runtime(value) => {
            json!({ "kind": "runtime", "value": value_json(value) })
        }
        CompletedHostBoundaryResult::Host(value) => {
            json!({ "kind": "host", "value": host_value_json(value) })
        }
    }
}

fn completed_host_boundary_result_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
) -> Result<CompletedHostBoundaryResult, InterpreterCodecError> {
    match required_str(value, "kind")? {
        "runtime" => Ok(CompletedHostBoundaryResult::Runtime(
            value_from_json_with_limits(limits, required_obj(value, "value")?)?,
        )),
        "host" => Ok(CompletedHostBoundaryResult::Host(host_value_from_json(
            required_obj(value, "value")?,
        )?)),
        other => Err(InterpreterCodecError::new(format!(
            "unsupported completed host boundary result kind `{other}`"
        ))),
    }
}

pub(super) fn compilation_identity_json(identity: &CheckpointCompilationIdentity) -> Value {
    json!({
        "schema_version": identity.schema_version,
        "compiler_version": identity.compiler_version,
        "project_fingerprint": identity.project_fingerprint,
        "checked_hir_fingerprint": identity.checked_hir_fingerprint,
        "dependency_metadata_fingerprints": identity
            .dependency_metadata_fingerprints
            .iter()
            .map(|(package, fingerprint)| json!({
                "package": package,
                "fingerprint": fingerprint,
            }))
            .collect::<Vec<_>>(),
        "entry_semantic_identity": identity.entry_semantic_identity,
    })
}

pub(super) fn compilation_identity_from_json(
    value: &Value,
) -> Result<CheckpointCompilationIdentity, InterpreterCodecError> {
    Ok(CheckpointCompilationIdentity {
        schema_version: required_str(value, "schema_version")?.to_owned(),
        compiler_version: required_str(value, "compiler_version")?.to_owned(),
        project_fingerprint: required_str(value, "project_fingerprint")?.to_owned(),
        checked_hir_fingerprint: required_str(value, "checked_hir_fingerprint")?.to_owned(),
        dependency_metadata_fingerprints: required_array(
            value,
            "dependency_metadata_fingerprints",
        )?
        .iter()
        .map(|record| {
            Ok((
                required_str(record, "package")?.to_owned(),
                required_str(record, "fingerprint")?.to_owned(),
            ))
        })
        .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        entry_semantic_identity: required_str(value, "entry_semantic_identity")?.to_owned(),
    })
}

pub(super) fn machine_json(machine: &MachineSnapshot) -> Result<Value, InterpreterCodecError> {
    let frames = machine
        .frames
        .iter()
        .map(|frame| -> Result<Value, InterpreterCodecError> {
            Ok(match frame {
                MachineFrameSnapshot::Block { continuation } => json!({
                    "kind": "block",
                    "continuation": machine_continuation_json(continuation)?,
                }),
                MachineFrameSnapshot::Expr { continuation } => json!({
                    "kind": "expr",
                    "continuation": machine_continuation_json(continuation)?,
                }),
                MachineFrameSnapshot::Call { continuation, span } => json!({
                    "kind": "call",
                    "continuation": machine_continuation_json(continuation)?,
                    "span": span_json(*span),
                }),
                MachineFrameSnapshot::Continuation { continuation } => json!({
                    "kind": "continuation",
                    "continuation": machine_continuation_json(continuation)?,
                }),
                MachineFrameSnapshot::Handler { continuation } => json!({
                    "kind": "handler",
                    "continuation": machine_continuation_json(continuation)?,
                }),
                MachineFrameSnapshot::Retry { continuation } => json!({
                    "kind": "retry",
                    "continuation": machine_continuation_json(continuation)?,
                }),
                MachineFrameSnapshot::ModelLoop(frame) => {
                    json!({
                        "kind": "model_loop",
                        "model": model_loop_frame_json(frame)?,
                    })
                }
                MachineFrameSnapshot::SourceToolReturn(frame) => {
                    json!({
                        "kind": "source_tool_return",
                        "source_tool": source_tool_return_frame_json(frame)?,
                    })
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({ "frames": frames }))
}

pub(super) fn machine_continuation_json(
    continuation: &ContinuationSnapshot,
) -> Result<Value, InterpreterCodecError> {
    let continuation = continuation
        .to_runtime()
        .map_err(InterpreterCodecError::new)?;
    machine::continuation_snapshot(&continuation).map_err(InterpreterCodecError::new)
}

pub(super) fn machine_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<MachineSnapshot, InterpreterCodecError> {
    let frames = value
        .get("frames")
        .and_then(Value::as_array)
        .ok_or_else(|| InterpreterCodecError::new("checkpoint machine is missing frame array"))?
        .iter()
        .map(|frame| {
            match required_str(frame, "kind")? {
                "model_loop" => {
                    let frame =
                        model_loop_frame_from_json(limits, required_obj(frame, "model")?, checked)?;
                    return Ok(MachineFrameSnapshot::ModelLoop(Box::new(frame)));
                }
                "source_tool_return" => {
                    let frame = source_tool_return_frame_from_json(
                        limits,
                        required_obj(frame, "source_tool")?,
                        checked,
                    )?;
                    return Ok(MachineFrameSnapshot::SourceToolReturn(frame));
                }
                _ => {}
            }
            let continuation = machine::continuation_from_snapshot(
                limits,
                required_obj(frame, "continuation")?,
                checked,
                std::sync::Arc::new(crate::plan::SlotLayoutTable::default()),
            )
            .map_err(InterpreterCodecError::new)?;
            let kind = required_str(frame, "kind")?;
            if kind == "handler"
                && !matches!(
                    continuation,
                    crate::control::Continuation::HandleBoundary { .. }
                )
            {
                return Err(InterpreterCodecError::new(
                    "checkpoint handler frame does not contain a handler boundary",
                ));
            }
            if kind == "retry"
                && !matches!(
                    continuation,
                    crate::control::Continuation::RetryAttempt { .. }
                )
            {
                return Err(InterpreterCodecError::new(
                    "checkpoint retry frame does not contain a retry attempt",
                ));
            }
            let continuation =
                ContinuationSnapshot::capture(&continuation).map_err(InterpreterCodecError::new)?;
            match kind {
                "block" => Ok(MachineFrameSnapshot::Block { continuation }),
                "expr" => Ok(MachineFrameSnapshot::Expr { continuation }),
                "call" => Ok(MachineFrameSnapshot::Call {
                    continuation,
                    span: span_from_json(required_obj(frame, "span")?)?,
                }),
                "continuation" => Ok(MachineFrameSnapshot::Continuation { continuation }),
                "handler" => Ok(MachineFrameSnapshot::Handler { continuation }),
                "retry" => Ok(MachineFrameSnapshot::Retry { continuation }),
                other => Err(InterpreterCodecError::new(format!(
                    "unsupported checkpoint machine frame `{other}`"
                ))),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MachineSnapshot { frames })
}

pub(super) fn model_loop_frame_json(
    frame: &crate::orchestration::ModelLoopFrameSnapshot,
) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "pending": pending_model_json(&frame.pending)?,
        "round": frame.round,
        "repair_attempts": frame.repair.attempts,
        "repair_kind": frame.repair.last_kind,
        "last_tool_error": frame.last_tool_error,
        "remaining_tool_calls": frame.remaining_tool_calls.iter().map(model_host_tool_call_json).collect::<Vec<_>>(),
        "completed_tool_result": frame.completed_tool_result,
        "current_host_tool": frame.current_host_tool.as_ref().map(|tool| json!({
            "call": model_host_tool_call_json(&tool.call),
            "boundary_key": tool.boundary_key,
        })),
        "boundary_key": frame.boundary_key,
        "outer_continuation": machine_continuation_json(&frame.outer_continuation)?,
    }))
}

pub(super) fn model_loop_frame_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<crate::orchestration::ModelLoopFrameSnapshot, InterpreterCodecError> {
    let current_host_tool = match value.get("current_host_tool") {
        None | Some(Value::Null) => None,
        Some(tool) => Some(crate::orchestration::HostToolProgressSnapshot {
            call: model_host_tool_call_from_json(required_obj(tool, "call")?)?,
            boundary_key: required_str(tool, "boundary_key")?.to_owned(),
        }),
    };
    Ok(crate::orchestration::ModelLoopFrameSnapshot {
        pending: pending_model_from_json(limits, required_obj(value, "pending")?, checked)?,
        round: required_usize(value, "round")?,
        repair: crate::orchestration::ModelRepairSnapshot {
            attempts: required_usize(value, "repair_attempts")?,
            last_kind: required_optional_string(value, "repair_kind")?,
        },
        last_tool_error: required_optional_string(value, "last_tool_error")?,
        remaining_tool_calls: required_array(value, "remaining_tool_calls")?
            .iter()
            .map(model_host_tool_call_from_json)
            .collect::<Result<Vec<_>, _>>()?,
        completed_tool_result: required_bool(value, "completed_tool_result")?,
        current_host_tool,
        boundary_key: required_str(value, "boundary_key")?.to_owned(),
        outer_continuation: ContinuationSnapshot::capture(
            &machine::continuation_from_snapshot(
                limits,
                required_obj(value, "outer_continuation")?,
                checked,
                std::sync::Arc::new(crate::plan::SlotLayoutTable::default()),
            )
            .map_err(InterpreterCodecError::new)?,
        )
        .map_err(InterpreterCodecError::new)?,
    })
}

pub(super) fn source_tool_return_frame_json(
    frame: &crate::orchestration::SourceToolReturnFrameSnapshot,
) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "tool_call_id": frame.tool_call_id,
        "tool_name": frame.tool_name,
        "binding": source_tool_binding_json(&frame.binding),
        "args": host_value_json(&frame.args),
        "boundary_key": frame.boundary_key,
        "output_schema": frame.output_schema.as_ref().map(machine::host_schema_snapshot),
        "model_loop": model_loop_frame_json(&frame.model_loop)?,
    }))
}

pub(super) fn source_tool_return_frame_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<crate::orchestration::SourceToolReturnFrameSnapshot, InterpreterCodecError> {
    Ok(crate::orchestration::SourceToolReturnFrameSnapshot {
        tool_call_id: required_str(value, "tool_call_id")?.to_owned(),
        tool_name: required_str(value, "tool_name")?.to_owned(),
        binding: source_tool_binding_from_json(required_obj(value, "binding")?)?,
        args: host_value_from_json(required_obj(value, "args")?)?,
        boundary_key: required_str(value, "boundary_key")?.to_owned(),
        output_schema: value
            .get("output_schema")
            .filter(|value| !value.is_null())
            .map(|value| {
                machine::host_schema_from_snapshot(value).map_err(InterpreterCodecError::new)
            })
            .transpose()?,
        model_loop: Box::new(model_loop_frame_from_json(
            limits,
            required_obj(value, "model_loop")?,
            checked,
        )?),
    })
}

pub(super) fn pending_model_json(
    pending: &crate::orchestration::PendingModelSnapshot,
) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "request": model_request_json(&pending.request)?,
        "decode": match pending.decode {
            crate::orchestration::ModelDecodeSnapshot::String => json!({"kind": "string"}),
            crate::orchestration::ModelDecodeSnapshot::ModelResponse => json!({"kind": "model_response"}),
            crate::orchestration::ModelDecodeSnapshot::Typed(ty) => json!({"kind": "typed", "type": ty.0}),
        },
        "max_tool_rounds": pending.max_tool_rounds,
        "source_tools": pending.source_tools.iter().map(source_tool_binding_json).collect::<Vec<_>>(),
        "span": span_json(pending.span),
        "continuation": machine_continuation_json(&pending.continuation)?,
    }))
}

pub(super) fn pending_model_from_json(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<crate::orchestration::PendingModelSnapshot, InterpreterCodecError> {
    let decode = required_obj(value, "decode")?;
    let decode = match required_str(decode, "kind")? {
        "string" => crate::orchestration::ModelDecodeSnapshot::String,
        "model_response" => crate::orchestration::ModelDecodeSnapshot::ModelResponse,
        "typed" => {
            crate::orchestration::ModelDecodeSnapshot::Typed(TypeId(required_u32(decode, "type")?))
        }
        other => {
            return Err(InterpreterCodecError::new(format!(
                "unsupported checkpoint model decode `{other}`"
            )));
        }
    };
    Ok(crate::orchestration::PendingModelSnapshot {
        request: model_request_from_json(required_obj(value, "request")?)?,
        decode,
        max_tool_rounds: required_usize(value, "max_tool_rounds")?,
        source_tools: required_array(value, "source_tools")?
            .iter()
            .map(source_tool_binding_from_json)
            .collect::<Result<Vec<_>, _>>()?,
        span: span_from_json(required_obj(value, "span")?)?,
        continuation: ContinuationSnapshot::capture(
            &machine::continuation_from_snapshot(
                limits,
                required_obj(value, "continuation")?,
                checked,
                std::sync::Arc::new(crate::plan::SlotLayoutTable::default()),
            )
            .map_err(InterpreterCodecError::new)?,
        )
        .map_err(InterpreterCodecError::new)?,
    })
}

pub(super) fn source_tool_binding_json(
    binding: &crate::orchestration::SourceToolBindingSnapshot,
) -> Value {
    json!({
        "name": binding.name,
        "qualified_name": binding.qualified_name,
        "item": binding.item.0,
    })
}

pub(super) fn source_tool_binding_from_json(
    value: &Value,
) -> Result<crate::orchestration::SourceToolBindingSnapshot, InterpreterCodecError> {
    Ok(crate::orchestration::SourceToolBindingSnapshot {
        name: required_str(value, "name")?.to_owned(),
        qualified_name: required_optional_string(value, "qualified_name")?,
        item: HirItemId(required_u32(value, "item")?),
    })
}

pub(super) fn model_request_json(
    request: &crate::orchestration::ModelRequestSnapshot,
) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "id": request.id.0,
        "provider": request.provider.as_ref().map(|provider| provider.0.as_str()),
        "model": request.model.0,
        "messages": request.messages.iter().map(model_host_message_json).collect::<Vec<_>>(),
        "tools": request.tools.iter().map(machine::tool_schema_snapshot).collect::<Vec<_>>(),
        "tool_choice": machine::model_tool_choice_snapshot(&request.tool_choice),
        "response_schema": request.response_schema.as_ref().map(machine::host_schema_snapshot),
        "policy_ref": request.policy_ref.as_ref().map(host_value_json),
        "options": machine::model_options_snapshot(&request.options),
        "budget_limits": budget_json(&request.budget_limits),
    }))
}

pub(super) fn model_request_from_json(
    value: &Value,
) -> Result<crate::orchestration::ModelRequestSnapshot, InterpreterCodecError> {
    for forbidden in ["authority", "trace", "budget"] {
        if value.get(forbidden).is_some() {
            return Err(InterpreterCodecError::new(format!(
                "model request snapshot cannot restore `{forbidden}`; invocation state must be supplied by the current host"
            )));
        }
    }
    Ok(crate::orchestration::ModelRequestSnapshot {
        id: HostRequestId(required_u32(value, "id")?),
        provider: required_optional_string(value, "provider")?.map(ModelProviderId),
        model: ModelName(required_str(value, "model")?.to_owned()),
        messages: required_array(value, "messages")?
            .iter()
            .map(model_host_message_from_json)
            .collect::<Result<Vec<_>, _>>()?,
        tools: required_array(value, "tools")?
            .iter()
            .map(|tool| {
                machine::tool_schema_from_snapshot(tool).map_err(InterpreterCodecError::new)
            })
            .collect::<Result<Vec<_>, _>>()?,
        tool_choice: machine::model_tool_choice_from_snapshot(required_obj(value, "tool_choice")?)
            .map_err(InterpreterCodecError::new)?,
        response_schema: value
            .get("response_schema")
            .filter(|value| !value.is_null())
            .map(|schema| {
                machine::host_schema_from_snapshot(schema).map_err(InterpreterCodecError::new)
            })
            .transpose()?,
        policy_ref: value
            .get("policy_ref")
            .filter(|value| !value.is_null())
            .map(host_value_from_json)
            .transpose()?,
        options: machine::model_options_from_snapshot(required_obj(value, "options")?)
            .map_err(InterpreterCodecError::new)?,
        budget_limits: budget_from_json(required_obj(value, "budget_limits")?)?,
    })
}

pub(super) fn model_host_message_json(message: &ModelMessage) -> Value {
    json!({
        "role": match message.role {
            ModelRole::System => "system",
            ModelRole::User => "user",
            ModelRole::Assistant => "assistant",
            ModelRole::Tool => "tool",
        },
        "content": message.content.iter().map(|content| match content {
            ModelContent::Text(text) => json!({"kind": "text", "text": text}),
            ModelContent::Value(value) => json!({"kind": "value", "value": host_value_json(value)}),
        }).collect::<Vec<_>>(),
        "tool_call_id": message.tool_call_id,
        "tool_calls": message.tool_calls.iter().map(model_host_tool_call_json).collect::<Vec<_>>(),
    })
}

pub(super) fn model_host_message_from_json(
    value: &Value,
) -> Result<ModelMessage, InterpreterCodecError> {
    let role = match required_str(value, "role")? {
        "system" => ModelRole::System,
        "user" => ModelRole::User,
        "assistant" => ModelRole::Assistant,
        "tool" => ModelRole::Tool,
        other => {
            return Err(InterpreterCodecError::new(format!(
                "invalid model role `{other}`"
            )));
        }
    };
    Ok(ModelMessage {
        role,
        content: required_array(value, "content")?
            .iter()
            .map(|content| match required_str(content, "kind")? {
                "text" => Ok(ModelContent::Text(
                    required_str(content, "text")?.to_owned(),
                )),
                "value" => Ok(ModelContent::Value(host_value_from_json(required_obj(
                    content, "value",
                )?)?)),
                other => Err(InterpreterCodecError::new(format!(
                    "invalid model content `{other}`"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()?,
        tool_call_id: required_optional_string(value, "tool_call_id")?,
        tool_calls: required_array(value, "tool_calls")?
            .iter()
            .map(model_host_tool_call_from_json)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

pub(super) fn model_host_tool_call_json(call: &ModelToolCall) -> Value {
    json!({"id": call.id, "tool": call.tool, "args": host_value_json(&call.args)})
}

pub(super) fn model_host_tool_call_from_json(
    value: &Value,
) -> Result<ModelToolCall, InterpreterCodecError> {
    Ok(ModelToolCall {
        id: required_str(value, "id")?.to_owned(),
        tool: required_str(value, "tool")?.to_owned(),
        args: host_value_from_json(required_obj(value, "args")?)?,
    })
}

pub(super) fn handler_arm_json(handler: &ActiveHandlerArmRecord) -> Value {
    json!({
        "effect_segments": handler.effect_segments,
        "action": handler.action,
        "action_symbol": handler.action_symbol.map(|symbol| symbol.0),
        "type_args": handler.type_args.iter().map(|ty| ty.0).collect::<Vec<_>>(),
        "effect_type_args": handler.effect_type_args.iter().map(|ty| ty.0).collect::<Vec<_>>(),
        "patterns": handler.patterns.iter().map(|pat| pat.0).collect::<Vec<_>>(),
        "body": handler.body.0,
        "scope": handler.scope.0,
        "span": span_json(handler.span),
    })
}

pub(super) fn active_handler_from_json(
    value: &Value,
) -> Result<ActiveHandlerRecord, InterpreterCodecError> {
    Ok(ActiveHandlerRecord {
        id: HandlerScopeId(required_u32(value, "id")?),
        handled_actions: string_array(value, "handled_actions")?,
        handlers: required_array(value, "handlers")?
            .iter()
            .map(handler_arm_from_json)
            .collect::<Result<Vec<_>, InterpreterCodecError>>()?,
        span: span_from_json(required_obj(value, "span")?)?,
    })
}

pub(super) fn handler_arm_from_json(
    value: &Value,
) -> Result<ActiveHandlerArmRecord, InterpreterCodecError> {
    Ok(ActiveHandlerArmRecord {
        effect_segments: string_array(value, "effect_segments")?,
        action: required_str(value, "action")?.to_owned(),
        action_symbol: optional_u32(value, "action_symbol")?.map(SymbolId),
        type_args: u32_array(value, "type_args")?
            .into_iter()
            .map(HirTypeId)
            .collect(),
        effect_type_args: u32_array(value, "effect_type_args")?
            .into_iter()
            .map(TypeId)
            .collect(),
        patterns: u32_array(value, "patterns")?
            .into_iter()
            .map(HirPatId)
            .collect(),
        body: HirBlockId(required_u32(value, "body")?),
        scope: ScopeId(required_u32(value, "scope")?),
        span: span_from_json(required_obj(value, "span")?)?,
    })
}

pub(super) fn resource_version_from_json(
    value: &Value,
) -> Result<ResourceVersionRecord, InterpreterCodecError> {
    Ok(ResourceVersionRecord {
        resource: required_str(value, "resource")?.to_owned(),
        version: required_str(value, "version")?.to_owned(),
    })
}

fn checkpoint_host_state_json(state: &CheckpointHostState) -> Result<Value, InterpreterCodecError> {
    Ok(json!({
        "trace": trace_context_json(&state.trace),
        "budget": checkpoint_budget_snapshot_json(&state.budget),
    }))
}

fn checkpoint_host_state_from_json(
    value: &Value,
) -> Result<CheckpointHostState, InterpreterCodecError> {
    Ok(CheckpointHostState {
        trace: trace_context_from_json(required_obj(value, "trace")?)?,
        budget: checkpoint_budget_snapshot_from_json(required_obj(value, "budget")?)?,
    })
}

pub(super) fn trace_context_json(trace: &TraceContext) -> Value {
    json!({
        "trace_id": trace.trace_id.to_hex(),
        "parent_trace": trace.parent_trace.map(TraceId::to_hex),
        "parent_span": trace.parent_span.map(|span| span.0),
    })
}

pub(super) fn trace_context_from_json(
    value: &Value,
) -> Result<TraceContext, InterpreterCodecError> {
    Ok(TraceContext {
        trace_id: trace_id_from_json(value, "trace_id")?,
        parent_trace: optional_trace_id_from_json(value, "parent_trace")?,
        parent_span: optional_u32(value, "parent_span")?.map(TraceSpanId),
    })
}

fn trace_id_from_json(
    value: &Value,
    field: &'static str,
) -> Result<TraceId, InterpreterCodecError> {
    let encoded = required_str(value, field)?;
    TraceId::from_hex(encoded)
        .map_err(|reason| InterpreterCodecError::new(format!("invalid `{field}`: {reason}")))
}

fn optional_trace_id_from_json(
    value: &Value,
    field: &'static str,
) -> Result<Option<TraceId>, InterpreterCodecError> {
    match value.get(field) {
        Some(Value::Null) | None => Ok(None),
        Some(Value::String(encoded)) => TraceId::from_hex(encoded)
            .map(Some)
            .map_err(|reason| InterpreterCodecError::new(format!("invalid `{field}`: {reason}"))),
        Some(_) => Err(InterpreterCodecError::new(format!(
            "invalid `{field}`: expected a string or null"
        ))),
    }
}

pub(crate) fn budget_json(budget: &Budget) -> Value {
    json!({
        "tokens": budget.tokens.map(|tokens| json!({ "max_tokens": tokens.max_tokens })),
        "time": budget.time.map(|time| json!({ "max_millis": time.max_millis })),
        "cost": budget.cost.as_ref().map(|cost| {
            json!({ "max_micros": cost.max_micros.to_string(), "currency": cost.currency })
        }),
    })
}

pub(crate) fn budget_from_json(value: &Value) -> Result<Budget, InterpreterCodecError> {
    Ok(Budget {
        tokens: match value.get("tokens") {
            Some(Value::Null) | None => None,
            Some(value) => Some(TokenBudget {
                max_tokens: required_u64(value, "max_tokens")?,
            }),
        },
        time: match value.get("time") {
            Some(Value::Null) | None => None,
            Some(value) => Some(TimeBudget {
                max_millis: required_u64(value, "max_millis")?,
            }),
        },
        cost: match value.get("cost") {
            Some(Value::Null) | None => None,
            Some(value) => {
                Some(CostBudget {
                    max_micros: required_str(value, "max_micros")?.parse::<u128>().map_err(
                        |_| InterpreterCodecError::new("`max_micros` must be a u128 string"),
                    )?,
                    currency: required_str(value, "currency")?.to_owned(),
                })
            }
        },
    })
}

fn checkpoint_budget_snapshot_json(snapshot: &CheckpointBudgetSnapshot) -> Value {
    json!({
        "limits": budget_json(&snapshot.limits),
        "state": {
            "deadline_unix_millis": snapshot.state.deadline_unix_millis.map(|value| value.to_string()),
            "reserved_tokens": snapshot.state.reserved_tokens,
            "consumed_tokens": snapshot.state.consumed_tokens,
            "reserved_cost_micros": snapshot.state.reserved_cost_micros.to_string(),
            "consumed_cost_micros": snapshot.state.consumed_cost_micros.to_string(),
        },
    })
}

fn checkpoint_budget_snapshot_from_json(
    value: &Value,
) -> Result<CheckpointBudgetSnapshot, InterpreterCodecError> {
    let limits = budget_from_json(required_obj(value, "limits")?)?;
    let state = required_obj(value, "state")?;
    let deadline_unix_millis = match state.get("deadline_unix_millis") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| {
                    InterpreterCodecError::new(
                        "execution budget deadline must be encoded as a u128 string",
                    )
                })?
                .parse::<u128>()
                .map_err(|_| {
                    InterpreterCodecError::new(
                        "execution budget deadline must be encoded as a u128 string",
                    )
                })?,
        ),
    };
    let parse_u128 = |name: &'static str| -> Result<u128, InterpreterCodecError> {
        required_str(state, name)?.parse::<u128>().map_err(|_| {
            InterpreterCodecError::new(format!(
                "execution budget `{name}` must be encoded as a u128 string"
            ))
        })
    };
    Ok(CheckpointBudgetSnapshot {
        limits,
        state: ExecutionBudgetSnapshot {
            deadline_unix_millis,
            reserved_tokens: required_u64(state, "reserved_tokens")?,
            consumed_tokens: required_u64(state, "consumed_tokens")?,
            reserved_cost_micros: parse_u128("reserved_cost_micros")?,
            consumed_cost_micros: parse_u128("consumed_cost_micros")?,
        },
    })
}

pub(super) fn span_json(span: Span) -> Value {
    json!({
        "source": span.source.0,
        "start": span.range.start.0,
        "end": span.range.end.0,
    })
}

pub(super) fn diagnostic_summary_json(diagnostic: &Diagnostic) -> Value {
    json!({
        "severity": format!("{:?}", diagnostic.severity).to_lowercase(),
        "phase": format!("{:?}", diagnostic.phase).to_lowercase(),
        "code": format!("{:?}", diagnostic.code),
        "message": diagnostic.message,
    })
}

pub(super) fn span_from_json(value: &Value) -> Result<Span, InterpreterCodecError> {
    Ok(Span::new(
        SourceId(required_u32(value, "source")?),
        TextRange::new(
            TextSize(required_u32(value, "start")?),
            TextSize(required_u32(value, "end")?),
        ),
    ))
}

pub fn checkpoint_from_json(
    value: &Value,
    checked: &etas_frontend::CheckedProject,
) -> Result<InterpreterCheckpoint, InterpreterCodecError> {
    checkpoint_from_json_with_limits(&etas_host::StorageLimits::default(), value, checked)
}
