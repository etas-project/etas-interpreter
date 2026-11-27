use super::{
    RestoreContext, capture_context::CaptureContext, capture_model_policy, restore_model_policy,
};
mod capture;
mod restore;
#[cfg(test)]
mod tests;
use std::collections::HashSet;

use crate::control::Continuation;
use crate::eval::{LocalPlaceComponent, LocalPlaceSegment, SliceExprEval};
use crate::orchestration::{
    ContinuationSnapshot, LocalPlaceComponentSnapshot, LocalPlaceSegmentSnapshot,
    SliceExprEvalSnapshot, ValueSnapshot,
};

impl ContinuationSnapshot {
    fn capture_leaf(
        continuation: &Continuation,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        Ok(match continuation {
            Continuation::DefaultArguments(state) => {
                Self::DefaultArguments(Box::new(crate::orchestration::DefaultArgumentsSnapshot {
                    tool: state.tool,
                    expr: state.expr,
                    parameter_count: state.parameter_count,
                    slots: state.slots.clone(),
                    values: capture_values(&state.values, context)?,
                    defaults: state.defaults.clone(),
                    next_default: state.next_default,
                    destination: match &state.destination {
                        crate::eval::arguments::ArgumentDestination::Call { target } => {
                            crate::orchestration::ArgumentDestinationSnapshot::Call {
                                target: target
                                    .as_ref()
                                    .map(|target| context.call_target(target))
                                    .transpose()?,
                            }
                        }
                        crate::eval::arguments::ArgumentDestination::Perform {
                            action,
                            type_args,
                            source_args,
                        } => crate::orchestration::ArgumentDestinationSnapshot::Perform {
                            action: action.clone(),
                            type_args: type_args.clone(),
                            source_args: source_args.clone(),
                        },
                    },
                    span: state.span,
                    frame: context.frame(&state.frame)?,
                }))
            }
            Continuation::CompleteRecordConstruction(guard) => {
                guard.checkpoint()?;
                Self::CompleteRecordConstruction {
                    id: guard.id,
                    task: guard.task,
                    owner: guard.owner,
                }
            }
            Continuation::PublishAgentConstruction { .. } => {
                return Err("cannot checkpoint an unfinished Agent initialization".into());
            }
            Continuation::FinishReceiver { .. } | Continuation::CommitReceiver { .. } => {
                return Err("cannot checkpoint while an updating receiver is held".into());
            }
            Continuation::TaskResume(pending) => Self::TaskResume {
                id: context.task_suspension(pending)?,
            },
            Continuation::AgentControllerStep(state) => Self::AgentControllerStep(
                crate::orchestration::AgentControllerSnapshot::capture(state, context)?,
            ),
            Continuation::AgentControllerTool(state) => Self::AgentControllerTool(
                crate::orchestration::AgentControllerSnapshot::capture(state, context)?,
            ),
            Continuation::ToolInvocationStart(invocation) => Self::ToolInvocationStart(
                crate::orchestration::ToolInvocationSnapshot::capture(invocation, context)?,
            ),
            Continuation::ToolInvocationResult(invocation) => Self::ToolInvocationResult(
                crate::orchestration::ToolInvocationSnapshot::capture(invocation, context)?,
            ),
            Continuation::AgentExecutionState(state) => {
                Self::AgentExecutionState(Box::new(context.agent_state(state)))
            }
            Continuation::PackExistential { expr, frame } => Self::PackExistential {
                expr: *expr,
                frame: context.frame(frame)?,
            },
            Continuation::ValidateCompileResult { .. } => {
                return Err("pending compiler contract evidence must be restored from source in its locked definition environment, not an opaque checkpoint handle".into());
            }
            Continuation::ValidateFileOpenResult { .. } => {
                return Err("cannot checkpoint a pending native file capability contract".into());
            }
            Continuation::PriorityQueueUpdate(_) => {
                return Err(
                    "an in-progress pure priority comparison cannot cross a checkpoint boundary"
                        .into(),
                );
            }
            Continuation::OrderedSearch(_) => {
                return Err(
                    "an in-progress pure key comparison cannot cross a checkpoint boundary".into(),
                );
            }
            Continuation::ValueEquality(_) => {
                return Err(
                    "an in-progress pure value comparison cannot cross a checkpoint boundary"
                        .into(),
                );
            }
            Continuation::ModelStepProviderData(_) => {
                return Err("a checked model response is still being decoded".into());
            }
            Continuation::StorageTransfer(_) => {
                return Err(
                    "an in-progress pure storage value transfer cannot cross a checkpoint boundary"
                        .into(),
                );
            }
            Continuation::DirectoryListReduction(state) => Self::DirectoryListReduction {
                call: context.call_target(&crate::control::CallTarget::StdIntrinsic(
                    state.call.clone(),
                ))?,
                args: capture_values(&state.args, context)?,
                max_entries: state.max_entries,
                max_bytes: state.max_bytes,
                span: state.span,
            },
            Continuation::FileReadReduction(state) => Self::FileReadReduction {
                call: context.call_target(&crate::control::CallTarget::StdIntrinsic(
                    state.call.clone(),
                ))?,
                args: capture_values(&state.args, context)?,
                limit: state.limit,
                span: state.span,
            },
            Continuation::StreamReadReduction(state) => Self::StreamReadReduction {
                call: context.call_target(&crate::control::CallTarget::StdIntrinsic(
                    state.call.clone(),
                ))?,
                args: capture_values(&state.args, context)?,
                limit: state.limit,
                deadline_unix_ms: state.deadline_unix_ms,
                body: ValueSnapshot::Bytes(state.body.clone()),
                span: state.span,
            },
            Continuation::ProgramResume(pending) => Self::ProgramResume {
                id: context.suspension(pending)?,
            },
            Continuation::ResidualScope(scope) => Self::ResidualScope {
                scope: context.residual_scope(scope)?,
            },
            Continuation::ResidualOutcome(tag) => Self::ResidualOutcome {
                attempt: context.residual_attempt(tag)?,
            },
            Continuation::ScopeExit {
                block, keep, frame, ..
            } => {
                let mut keep = keep.iter().copied().collect::<Vec<_>>();
                keep.sort_by_key(|symbol| symbol.0);
                Self::ScopeExit {
                    block: *block,
                    keep,
                    frame: context.frame(frame)?,
                }
            }
            Continuation::ContinueBlock {
                block,
                next_stmt_index,
                frame,
            } => Self::ContinueBlock {
                block: *block,
                next_stmt_index: *next_stmt_index,
                frame: context.frame(frame)?,
            },
            Continuation::Bind {
                block,
                next_stmt_index,
                pat,
                span,
                frame,
            } => Self::Bind {
                block: *block,
                next_stmt_index: *next_stmt_index,
                pat: *pat,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::Assign {
                block,
                next_stmt_index,
                target,
                span,
                frame,
            } => Self::Assign {
                block: *block,
                next_stmt_index: *next_stmt_index,
                target: *target,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::AssignTargetIndex {
                block,
                next_stmt_index,
                root_symbol,
                segments,
                components,
                next_component_index,
                new_value,
                span,
                frame,
            } => Self::AssignTargetIndex {
                block: *block,
                next_stmt_index: *next_stmt_index,
                root_symbol: *root_symbol,
                segments: segments
                    .iter()
                    .map(|segment| capture_local_segment(segment, context))
                    .collect::<Result<Vec<_>, _>>()?,
                components: components.iter().map(capture_local_component).collect(),
                next_component_index: *next_component_index,
                new_value: context.value(new_value)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::FieldReceiver {
                expr,
                field,
                span,
                frame,
            } => Self::FieldReceiver {
                expr: *expr,
                field: field.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::Unary { op, span } => Self::Unary {
                op: *op,
                span: *span,
            },
            Continuation::BinaryLeft {
                op,
                rhs,
                span,
                frame,
            } => Self::BinaryLeft {
                op: *op,
                rhs: *rhs,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::BinaryRight { op, left, span } => Self::BinaryRight {
                op: *op,
                left: context.value(left)?,
                span: *span,
            },
            Continuation::AggregateElement {
                expr,
                next_index,
                values,
                frame,
            } => Self::AggregateElement {
                expr: *expr,
                next_index: *next_index,
                values: capture_values(values, context)?,
                frame: context.frame(frame)?,
            },
            Continuation::ListConsHead { tail, span, frame } => Self::ListConsHead {
                tail: *tail,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::ListConsTail { head, span } => Self::ListConsTail {
                head: context.value(head)?,
                span: *span,
            },
            Continuation::RangeStart { end, bounds, frame } => Self::RangeStart {
                end: *end,
                bounds: *bounds,
                frame: context.frame(frame)?,
            },
            Continuation::RangeEnd { start, bounds } => Self::RangeEnd {
                start: context.value(start)?,
                bounds: *bounds,
            },
            Continuation::RecordField {
                expr,
                nominal_type,
                variant_symbol,
                next_index,
                values,
                frame,
            } => Self::RecordField {
                expr: *expr,
                nominal_type: *nominal_type,
                variant_symbol: *variant_symbol,
                next_index: *next_index,
                values: values
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), context.value(value)?)))
                    .collect::<Result<Vec<_>, String>>()?,
                frame: context.frame(frame)?,
            },
            Continuation::MapKey {
                expr,
                index,
                values,
                frame,
            } => Self::MapKey {
                expr: *expr,
                index: *index,
                values: capture_pairs(values, context)?,
                frame: context.frame(frame)?,
            },
            Continuation::MapValue {
                expr,
                index,
                key,
                values,
                frame,
            } => Self::MapValue {
                expr: *expr,
                index: *index,
                key: context.value(key)?,
                values: capture_pairs(values, context)?,
                frame: context.frame(frame)?,
            },
            Continuation::IndexBase {
                expr,
                index,
                span,
                frame,
            } => Self::IndexBase {
                expr: *expr,
                index: *index,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::IndexValue {
                expr,
                base,
                span,
                frame,
            } => Self::IndexValue {
                expr: *expr,
                base: context.value(base)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::SliceBase { eval, frame } => Self::SliceBase {
                eval: capture_slice_eval(*eval),
                frame: context.frame(frame)?,
            },
            Continuation::SliceStart { eval, base, frame } => Self::SliceStart {
                eval: capture_slice_eval(*eval),
                base: context.value(base)?,
                frame: context.frame(frame)?,
            },
            Continuation::SliceEnd {
                eval,
                base,
                start,
                frame,
            } => Self::SliceEnd {
                eval: capture_slice_eval(*eval),
                base: context.value(base)?,
                start: context.value(start)?,
                frame: context.frame(frame)?,
            },
            Continuation::MethodReceiver {
                expr,
                method,
                type_args,
                args,
                span,
                frame,
            } => Self::MethodReceiver {
                expr: *expr,
                method: method.clone(),
                type_args: type_args.clone(),
                args: args.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::PromptValueMethodArg {
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            } => Self::PromptValueMethodArg {
                messages: messages.clone(),
                method: method.clone(),
                role: *role,
                allow_plain_system_content: *allow_plain_system_content,
                span: *span,
            },
            Continuation::CalleeEval {
                call,
                args,
                span,
                frame,
            } => Self::CalleeEval {
                call: *call,
                args: args.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::PipelineStageTarget {
                expr,
                next_stage_index,
                targets,
                span,
                frame,
            } => Self::PipelineStageTarget {
                expr: *expr,
                next_stage_index: *next_stage_index,
                targets: targets
                    .iter()
                    .map(|target| context.call_target(target))
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::CallArgs {
                call,
                target,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::CallArgs {
                call: *call,
                target: target
                    .as_ref()
                    .map(|target| context.call_target(target))
                    .transpose()?,
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::VariantArgs {
                variant_symbol,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::VariantArgs {
                variant_symbol: *variant_symbol,
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::PerformArgs {
                expr,
                action,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::PerformArgs {
                expr: *expr,
                action: action.as_ref().clone(),
                type_args: type_args.clone(),
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::IfExpr {
                then_block,
                else_branch,
                span,
                frame,
            } => Self::IfExpr {
                then_block: *then_block,
                else_branch: else_branch.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::IfStmt {
                block,
                next_stmt_index,
                then_block,
                else_branch,
                span,
                frame,
            } => Self::IfStmt {
                block: *block,
                next_stmt_index: *next_stmt_index,
                then_block: *then_block,
                else_branch: else_branch.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::MatchExpr { arms, span, frame } => Self::MatchExpr {
                arms: arms.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::MatchStmt {
                block,
                next_stmt_index,
                arms,
                span,
                frame,
            } => Self::MatchStmt {
                block: *block,
                next_stmt_index: *next_stmt_index,
                arms: arms.clone(),
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::HandleHandler {
                handle_expr,
                body,
                handler,
                span,
                frame,
            } => Self::HandleHandler {
                handle_expr: *handle_expr,
                body: *body,
                handler: *handler,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::PipelineInput { expr, span, frame } => Self::PipelineInput {
                expr: *expr,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::PipelineTarget {
                expr,
                input,
                span,
                frame,
            } => Self::PipelineTarget {
                expr: *expr,
                input: context.value(input)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::ComposedCall {
                remaining,
                next_index,
                span,
                site,
            } => Self::ComposedCall {
                site: site
                    .as_ref()
                    .map(|site| crate::orchestration::CallSiteSnapshot::capture(site, context))
                    .transpose()?,
                remaining: remaining
                    .get(*next_index..)
                    .ok_or("checkpoint composition cursor is out of bounds")?
                    .iter()
                    .map(|target| context.call_target(target))
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
            },
            Continuation::ForLoop {
                max_iterations,
                pat,
                source,
                next_index,
                body,
                loop_scope,
                span,
                frame,
            } => {
                let mut loop_scope = loop_scope.iter().copied().collect::<Vec<_>>();
                loop_scope.sort_by_key(|symbol| symbol.0);
                Self::ForLoop {
                    max_iterations: *max_iterations,
                    pat: *pat,
                    source: source
                        .as_ref()
                        .map(|source| context.value(source.value()))
                        .transpose()?,
                    text_cursor: source
                        .as_ref()
                        .map(|source| source.text_cursor(*next_index))
                        .transpose()?
                        .flatten(),
                    next_index: *next_index,
                    body: *body,
                    loop_scope,
                    span: *span,
                    frame: context.frame(frame)?,
                }
            }
            Continuation::SourceForLoop {
                max_iterations,
                iter,
                pat,
                source,
                awaiting_step,
                next_index,
                body,
                loop_scope,
                span,
                frame,
            } => {
                let mut loop_scope = loop_scope.iter().copied().collect::<Vec<_>>();
                loop_scope.sort_by_key(|symbol| symbol.0);
                Self::SourceForLoop {
                    max_iterations: *max_iterations,
                    iter: *iter,
                    pat: *pat,
                    source: source
                        .as_deref()
                        .map(|value| context.value(value))
                        .transpose()?,
                    awaiting_step: *awaiting_step,
                    next_index: *next_index,
                    body: *body,
                    loop_scope,
                    span: *span,
                    frame: context.frame(frame)?,
                }
            }
            Continuation::WhileLoop {
                iteration_budget,
                cond,
                body,
                resume_after_body,
                span,
                frame,
            } => Self::WhileLoop {
                iteration_budget: *iteration_budget,
                cond: *cond,
                body: *body,
                resume_after_body: *resume_after_body,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::RetryAttempt {
                max_attempts,
                retry,
                body,
                next_attempt,
                block,
                next_stmt_index,
                frame,
            } => Self::RetryAttempt {
                max_attempts: *max_attempts,
                retry: retry.clone(),
                body: *body,
                next_attempt: *next_attempt,
                block: *block,
                next_stmt_index: *next_stmt_index,
                frame: context.frame(frame)?,
            },
            Continuation::TryExpr {
                expr,
                result_type,
                span,
            } => Self::TryExpr {
                expr: *expr,
                result_type: *result_type,
                span: *span,
            },
            Continuation::MemoryClearDeleteAll {
                region_stable_id,
                path,
                span,
            } => Self::MemoryClearDeleteAll {
                region_stable_id: region_stable_id.clone(),
                path: path.clone(),
                span: *span,
            },
            Continuation::MemoryClearDeleteNext {
                region_stable_id,
                path,
                remaining_keys,
                next_index,
                span,
            } => Self::MemoryClearDeleteNext {
                region_stable_id: region_stable_id.clone(),
                path: path.clone(),
                remaining_keys: capture_values(remaining_keys, context)?,
                next_index: *next_index,
                span: *span,
            },
            Continuation::AgentInference {
                expr,
                span,
                model_policy,
            } => Self::AgentInference {
                expr: *expr,
                span: *span,
                model_policy: Box::new(capture_model_policy(model_policy)),
            },
            Continuation::RestoreModelPolicy { .. }
            | Continuation::CallBoundary { .. }
            | Continuation::HandlerDispatch { .. }
            | Continuation::ReenterHandler { .. }
            | Continuation::HandleBoundary { .. }
            | Continuation::AgentExecutionScope { .. }
            | Continuation::ScopedModelPolicy { .. }
            | Continuation::Chain { .. } => {
                return Err("continuation capture expected a leaf, not an owned edge".into());
            }
            Continuation::Return => Self::Return,
            Continuation::Resume => Self::Resume,
            Continuation::Finish => Self::Finish,
            Continuation::RestoreHandlerArms { scope_id, handlers } => Self::RestoreHandlerArms {
                scope_id: *scope_id,
                handlers: handlers.clone(),
            },
            Continuation::BlockValue => Self::BlockValue,
        })
    }

    fn restore_leaf(self, context: &mut RestoreContext) -> Result<Continuation, String> {
        Ok(match self {
            Self::DefaultArguments(state) => {
                let state = *state;
                Continuation::DefaultArguments(Box::new(crate::eval::arguments::DefaultArguments {
                    tool: state.tool,
                    expr: state.expr,
                    parameter_count: state.parameter_count,
                    slots: state.slots,
                    values: restore_values(state.values, context)?,
                    defaults: state.defaults,
                    next_default: state.next_default,
                    destination: match state.destination {
                        crate::orchestration::ArgumentDestinationSnapshot::Call { target } => {
                            crate::eval::arguments::ArgumentDestination::Call {
                                target: target
                                    .map(|target| {
                                        super::call_target::restore_call_target(target, context)
                                    })
                                    .transpose()?,
                            }
                        }
                        crate::orchestration::ArgumentDestinationSnapshot::Perform {
                            action,
                            type_args,
                            source_args,
                        } => crate::eval::arguments::ArgumentDestination::Perform {
                            action,
                            type_args,
                            source_args,
                        },
                    },
                    span: state.span,
                    frame: super::frame::restore_frame(state.frame, context)?,
                }))
            }
            Self::CompleteRecordConstruction { id, task, owner } => {
                Continuation::CompleteRecordConstruction(
                    context.record_construction(id, task, owner)?,
                )
            }
            Self::ToolInvocationStart(invocation) => {
                Continuation::ToolInvocationStart(invocation.restore(context)?)
            }
            Self::ToolInvocationResult(invocation) => {
                Continuation::ToolInvocationResult(invocation.restore(context)?)
            }
            Self::AgentExecutionState(state) => {
                Continuation::AgentExecutionState(Box::new(context.agent_state(*state)?))
            }
            Self::PackExistential { expr, frame } => Continuation::PackExistential {
                expr,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::ProgramResume { id } => Continuation::ProgramResume(context.suspension(id)?),
            Self::TaskResume { id } => Continuation::TaskResume(context.task_suspension(id)?),
            Self::ResidualScope { scope } => {
                Continuation::ResidualScope(context.residual_scope(scope)?)
            }
            Self::ResidualOutcome { attempt } => {
                Continuation::ResidualOutcome(context.residual_attempt(attempt)?)
            }
            Self::DirectoryListReduction {
                call,
                args,
                max_entries,
                max_bytes,
                span,
            } => {
                let crate::control::CallTarget::StdIntrinsic(call) =
                    super::call_target::restore_call_target(call, context)?
                else {
                    return Err("bounded directory reducer requires a checked intrinsic".into());
                };
                Continuation::DirectoryListReduction(Box::new(
                    crate::control::DirectoryListReduction {
                        call,
                        args: restore_values(args, context)?,
                        max_entries,
                        max_bytes,
                        span,
                    },
                ))
            }
            Self::FileReadReduction {
                call,
                args,
                limit,
                span,
            } => {
                let crate::control::CallTarget::StdIntrinsic(call) =
                    super::call_target::restore_call_target(call, context)?
                else {
                    return Err("bounded file reducer requires a checked standard intrinsic".into());
                };
                Continuation::FileReadReduction(Box::new(crate::control::FileReadReduction {
                    call,
                    args: restore_values(args, context)?,
                    limit,
                    span,
                }))
            }
            Self::StreamReadReduction {
                call,
                args,
                limit,
                deadline_unix_ms,
                body,
                span,
            } => {
                let crate::control::CallTarget::StdIntrinsic(call) =
                    super::call_target::restore_call_target(call, context)?
                else {
                    return Err("stream reducer requires a checked standard intrinsic".into());
                };
                let ValueSnapshot::Bytes(body) = body else {
                    return Err("stream reducer buffer must be bytes".into());
                };
                Continuation::StreamReadReduction(Box::new(crate::control::StreamReadReduction {
                    call,
                    args: restore_values(args, context)?,
                    limit,
                    deadline_unix_ms,
                    body,
                    span,
                }))
            }
            Self::ScopeExit { block, keep, frame } => {
                let frame = super::frame::restore_frame(frame, context)?;
                let keep =
                    std::rc::Rc::new(keep.into_iter().collect::<std::collections::HashSet<_>>());
                let guard = context.scope_exit_guard(block, &frame, keep.clone())?;
                Continuation::ScopeExit {
                    block,
                    keep,
                    frame,
                    guard: Some(guard),
                }
            }
            Self::ContinueBlock {
                block,
                next_stmt_index,
                frame,
            } => Continuation::ContinueBlock {
                block,
                next_stmt_index,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::Bind {
                block,
                next_stmt_index,
                pat,
                span,
                frame,
            } => Continuation::Bind {
                block,
                next_stmt_index,
                pat,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::Assign {
                block,
                next_stmt_index,
                target,
                span,
                frame,
            } => Continuation::Assign {
                block,
                next_stmt_index,
                target,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::AssignTargetIndex {
                block,
                next_stmt_index,
                root_symbol,
                segments,
                components,
                next_component_index,
                new_value,
                span,
                frame,
            } => Continuation::AssignTargetIndex {
                block,
                next_stmt_index,
                root_symbol,
                segments: segments
                    .into_iter()
                    .map(|segment| restore_local_segment(segment, context))
                    .collect::<Result<Vec<_>, _>>()?,
                components: components
                    .into_iter()
                    .map(restore_local_component)
                    .collect(),
                next_component_index,
                new_value: Box::new(new_value.restore_with(context)?),
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::FieldReceiver {
                expr,
                field,
                span,
                frame,
            } => Continuation::FieldReceiver {
                expr,
                field,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::Unary { op, span } => Continuation::Unary { op, span },
            Self::BinaryLeft {
                op,
                rhs,
                span,
                frame,
            } => Continuation::BinaryLeft {
                op,
                rhs,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::BinaryRight { op, left, span } => Continuation::BinaryRight {
                op,
                left: Box::new(left.restore_with(context)?),
                span,
            },
            Self::AggregateElement {
                expr,
                next_index,
                values,
                frame,
            } => Continuation::AggregateElement {
                expr,
                next_index,
                values: restore_values(values, context)?,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::ListConsHead { tail, span, frame } => Continuation::ListConsHead {
                tail,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::ListConsTail { head, span } => Continuation::ListConsTail {
                head: Box::new(head.restore_with(context)?),
                span,
            },
            Self::RangeStart { end, bounds, frame } => Continuation::RangeStart {
                end,
                bounds,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::RangeEnd { start, bounds } => Continuation::RangeEnd {
                start: Box::new(start.restore_with(context)?),
                bounds,
            },
            Self::RecordField {
                expr,
                nominal_type,
                variant_symbol,
                next_index,
                values,
                frame,
            } => Continuation::RecordField {
                expr,
                nominal_type,
                variant_symbol,
                next_index,
                values: values
                    .into_iter()
                    .map(|(name, value)| Ok((name, value.restore_with(context)?)))
                    .collect::<Result<Vec<_>, String>>()?,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MapKey {
                expr,
                index,
                values,
                frame,
            } => Continuation::MapKey {
                expr,
                index,
                values: restore_pairs(values, context)?,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MapValue {
                expr,
                index,
                key,
                values,
                frame,
            } => Continuation::MapValue {
                expr,
                index,
                key: Box::new(key.restore_with(context)?),
                values: restore_pairs(values, context)?,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::IndexBase {
                expr,
                index,
                span,
                frame,
            } => Continuation::IndexBase {
                expr,
                index,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::IndexValue {
                expr,
                base,
                span,
                frame,
            } => Continuation::IndexValue {
                expr,
                base: Box::new(base.restore_with(context)?),
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SliceBase { eval, frame } => Continuation::SliceBase {
                eval: restore_slice_eval(eval),
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SliceStart { eval, base, frame } => Continuation::SliceStart {
                eval: restore_slice_eval(eval),
                base: Box::new(base.restore_with(context)?),
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SliceEnd {
                eval,
                base,
                start,
                frame,
            } => Continuation::SliceEnd {
                eval: restore_slice_eval(eval),
                base: Box::new(base.restore_with(context)?),
                start: Box::new(start.restore_with(context)?),
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MethodReceiver {
                expr,
                method,
                type_args,
                args,
                span,
                frame,
            } => Continuation::MethodReceiver {
                expr,
                method,
                type_args,
                args,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::PromptValueMethodArg {
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            } => Continuation::PromptValueMethodArg {
                messages,
                method,
                role,
                allow_plain_system_content,
                span,
            },
            Self::CalleeEval {
                call,
                args,
                span,
                frame,
            } => Continuation::CalleeEval {
                call,
                args,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::PipelineStageTarget {
                expr,
                next_stage_index,
                targets,
                span,
                frame,
            } => Continuation::PipelineStageTarget {
                expr,
                next_stage_index,
                targets: targets
                    .into_iter()
                    .map(|target| super::call_target::restore_call_target(target, context))
                    .collect::<Result<Vec<_>, _>>()?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::CallArgs {
                call,
                target,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::CallArgs {
                call,
                target: target
                    .map(|target| super::call_target::restore_call_target(target, context))
                    .transpose()?,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::VariantArgs {
                variant_symbol,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::VariantArgs {
                variant_symbol,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::PerformArgs {
                expr,
                action,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::PerformArgs {
                expr,
                action: Box::new(action),
                type_args,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::IfExpr {
                then_block,
                else_branch,
                span,
                frame,
            } => Continuation::IfExpr {
                then_block,
                else_branch,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::IfStmt {
                block,
                next_stmt_index,
                then_block,
                else_branch,
                span,
                frame,
            } => Continuation::IfStmt {
                block,
                next_stmt_index,
                then_block,
                else_branch,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MatchExpr { arms, span, frame } => Continuation::MatchExpr {
                arms,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MatchStmt {
                block,
                next_stmt_index,
                arms,
                span,
                frame,
            } => Continuation::MatchStmt {
                block,
                next_stmt_index,
                arms,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::HandleHandler {
                handle_expr,
                body,
                handler,
                span,
                frame,
            } => Continuation::HandleHandler {
                handle_expr,
                body,
                handler,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::PipelineInput { expr, span, frame } => Continuation::PipelineInput {
                expr,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::PipelineTarget {
                expr,
                input,
                span,
                frame,
            } => Continuation::PipelineTarget {
                expr,
                input: Box::new(input.restore_with(context)?),
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::ComposedCall {
                remaining,
                span,
                site,
            } => Continuation::ComposedCall {
                site: site.map(|site| site.restore(context)).transpose()?,
                next_index: 0,
                remaining: remaining
                    .into_iter()
                    .map(|target| super::call_target::restore_call_target(target, context))
                    .collect::<Result<Vec<_>, _>>()?
                    .into(),
                span,
            },
            Self::ForLoop {
                max_iterations,
                pat,
                source,
                text_cursor,
                next_index,
                body,
                loop_scope,
                span,
                frame,
            } => Continuation::ForLoop {
                max_iterations,
                pat,
                source: match (source, text_cursor) {
                    (Some(source), Some(cursor)) => Some(Box::new(
                        crate::value::iteration::IterationSource::restore_text_cursor(
                            source.restore_with(context)?,
                            next_index,
                            cursor,
                        )?,
                    )),
                    (Some(source), None) => {
                        Some(Box::new(crate::value::iteration::IterationSource::new(
                            source.restore_with(context)?,
                        )?))
                    }
                    (None, None) => None,
                    (None, Some(_)) => {
                        return Err("checkpoint text cursor has no iteration source".into());
                    }
                },
                next_index,
                body,
                loop_scope: loop_scope.into_iter().collect::<HashSet<_>>(),
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SourceForLoop {
                max_iterations,
                iter,
                pat,
                source,
                awaiting_step,
                next_index,
                body,
                loop_scope,
                span,
                frame,
            } => Continuation::SourceForLoop {
                max_iterations,
                iter,
                pat,
                source: source
                    .map(|value| value.restore_with(context).map(Box::new))
                    .transpose()?,
                awaiting_step,
                next_index,
                body,
                loop_scope: loop_scope.into_iter().collect(),
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::WhileLoop {
                iteration_budget,
                cond,
                body,
                resume_after_body,
                span,
                frame,
            } => Continuation::WhileLoop {
                iteration_budget,
                cond,
                body,
                resume_after_body,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::RetryAttempt {
                max_attempts,
                retry,
                body,
                next_attempt,
                block,
                next_stmt_index,
                frame,
            } => Continuation::RetryAttempt {
                max_attempts,
                retry,
                body,
                next_attempt,
                block,
                next_stmt_index,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::TryExpr {
                expr,
                result_type,
                span,
            } => Continuation::TryExpr {
                expr,
                result_type,
                span,
            },
            Self::MemoryClearDeleteAll {
                region_stable_id,
                path,
                span,
            } => Continuation::MemoryClearDeleteAll {
                region_stable_id,
                path,
                span,
            },
            Self::MemoryClearDeleteNext {
                region_stable_id,
                path,
                remaining_keys,
                next_index,
                span,
            } => Continuation::MemoryClearDeleteNext {
                region_stable_id,
                path,
                remaining_keys: restore_values(remaining_keys, context)?,
                next_index,
                span,
            },
            Self::AgentControllerStep(state) => {
                Continuation::AgentControllerStep(state.restore(context)?)
            }
            Self::AgentControllerTool(state) => {
                Continuation::AgentControllerTool(state.restore(context)?)
            }
            Self::AgentInference {
                expr,
                span,
                model_policy,
            } => Continuation::AgentInference {
                expr,
                span,
                model_policy: Box::new(restore_model_policy(*model_policy)),
            },
            Self::RestoreModelPolicy { .. }
            | Self::CallBoundary { .. }
            | Self::HandlerDispatch { .. }
            | Self::ReenterHandler { .. }
            | Self::HandleBoundary { .. }
            | Self::AgentExecutionScope { .. }
            | Self::ScopedModelPolicy { .. }
            | Self::Chain { .. } => {
                return Err("continuation restore expected a leaf, not an owned edge".into());
            }
            Self::Return => Continuation::Return,
            Self::Resume => Continuation::Resume,
            Self::Finish => Continuation::Finish,
            Self::RestoreHandlerArms { scope_id, handlers } => {
                Continuation::RestoreHandlerArms { scope_id, handlers }
            }
            Self::BlockValue => Continuation::BlockValue,
        })
    }
}

fn capture_values(
    values: &[crate::value::InterpValue],
    context: &mut CaptureContext,
) -> Result<Vec<ValueSnapshot>, String> {
    values.iter().map(|value| context.value(value)).collect()
}

fn restore_values(
    values: Vec<ValueSnapshot>,
    context: &mut RestoreContext,
) -> Result<Vec<crate::value::InterpValue>, String> {
    values
        .into_iter()
        .map(|value| value.restore_with(context))
        .collect()
}

fn capture_pairs(
    values: &[(crate::value::InterpValue, crate::value::InterpValue)],
    context: &mut CaptureContext,
) -> Result<Vec<(ValueSnapshot, ValueSnapshot)>, String> {
    values
        .iter()
        .map(|(key, value)| Ok((context.value(key)?, context.value(value)?)))
        .collect()
}

fn restore_pairs(
    values: Vec<(ValueSnapshot, ValueSnapshot)>,
    context: &mut RestoreContext,
) -> Result<Vec<(crate::value::InterpValue, crate::value::InterpValue)>, String> {
    values
        .into_iter()
        .map(|(key, value)| Ok((key.restore_with(context)?, value.restore_with(context)?)))
        .collect()
}

fn capture_local_segment(
    value: &LocalPlaceSegment,
    context: &mut CaptureContext,
) -> Result<LocalPlaceSegmentSnapshot, String> {
    Ok(match value {
        LocalPlaceSegment::Field(value) => LocalPlaceSegmentSnapshot::Field(value.clone()),
        LocalPlaceSegment::Index(value) => LocalPlaceSegmentSnapshot::Index(*value),
        LocalPlaceSegment::MapKey(value) => {
            LocalPlaceSegmentSnapshot::MapKey(Box::new(context.value(value)?))
        }
    })
}

fn restore_local_segment(
    value: LocalPlaceSegmentSnapshot,
    context: &mut RestoreContext,
) -> Result<LocalPlaceSegment, String> {
    Ok(match value {
        LocalPlaceSegmentSnapshot::Field(value) => LocalPlaceSegment::Field(value),
        LocalPlaceSegmentSnapshot::Index(value) => LocalPlaceSegment::Index(value),
        LocalPlaceSegmentSnapshot::MapKey(value) => {
            LocalPlaceSegment::MapKey(Box::new(value.restore_with(context)?))
        }
    })
}

fn capture_local_component(value: &LocalPlaceComponent) -> LocalPlaceComponentSnapshot {
    match value {
        LocalPlaceComponent::Field(value) => LocalPlaceComponentSnapshot::Field(value.clone()),
        LocalPlaceComponent::Index { base, index } => LocalPlaceComponentSnapshot::Index {
            base: *base,
            index: *index,
        },
    }
}

fn restore_local_component(value: LocalPlaceComponentSnapshot) -> LocalPlaceComponent {
    match value {
        LocalPlaceComponentSnapshot::Field(value) => LocalPlaceComponent::Field(value),
        LocalPlaceComponentSnapshot::Index { base, index } => {
            LocalPlaceComponent::Index { base, index }
        }
    }
}

fn capture_slice_eval(value: SliceExprEval) -> SliceExprEvalSnapshot {
    SliceExprEvalSnapshot {
        expr: value.expr,
        base: value.base,
        start: value.start,
        end: value.end,
        bounds: value.bounds,
        span: value.span,
    }
}

fn restore_slice_eval(value: SliceExprEvalSnapshot) -> SliceExprEval {
    SliceExprEval {
        expr: value.expr,
        base: value.base,
        start: value.start,
        end: value.end,
        bounds: value.bounds,
        span: value.span,
    }
}
