use super::{
    RestoreContext, capture_context::CaptureContext, capture_model_policy, restore_model_policy,
};
mod capture;
mod restore;
#[cfg(test)]
mod tests;
use std::collections::HashSet;

use crate::control::{Continuation, StaticMethodKind};
use crate::eval::{LocalPlaceComponent, LocalPlaceSegment, SliceExprEval};
use crate::orchestration::{
    ContinuationSnapshot, LocalPlaceComponentSnapshot, LocalPlaceSegmentSnapshot,
    SliceExprEvalSnapshot, StaticMethodKindSnapshot, ValueSnapshot,
};

impl ContinuationSnapshot {
    fn capture_leaf(
        continuation: &Continuation,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        Ok(match continuation {
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
            Continuation::LocalMethodArgs {
                expr,
                receiver,
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::LocalMethodArgs {
                expr: *expr,
                receiver: context.value(receiver)?,
                method: method.clone(),
                type_args: type_args.clone(),
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::StaticMethodArgs {
                expr,
                kind,
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::StaticMethodArgs {
                expr: *expr,
                kind: capture_static_method_kind(kind),
                method: method.clone(),
                type_args: type_args.clone(),
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::SpecMethodReceiver {
                expr,
                receiver_expr,
                spec_symbol,
                spec_args,
                method,
                args,
                span,
                frame,
            } => Self::SpecMethodReceiver {
                expr: *expr,
                receiver_expr: *receiver_expr,
                spec_symbol: *spec_symbol,
                spec_args: spec_args.clone(),
                method: method.clone(),
                args: args.clone(),
                span: *span,
                frame: context.frame(frame)?,
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
                target: context.call_target(target)?,
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
                action: action.clone(),
                type_args: type_args.clone(),
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::MemoryArgs {
                region_stable_id,
                path,
                key_type,
                value_type,

                result_type,
                method,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::MemoryArgs {
                region_stable_id: region_stable_id.clone(),
                path: path.clone(),
                key_type: *key_type,
                value_type: *value_type,
                result_type: *result_type,
                method: method.clone(),
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args, context)?,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::MemorySelectionLimitArgs {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate,
                limit,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::MemorySelectionLimitArgs {
                region_stable_id: region_stable_id.clone(),
                path: path.clone(),
                key_type: *key_type,
                value_type: *value_type,
                kind: kind.clone(),
                predicate: predicate
                    .as_ref()
                    .map(|value| context.value(value))
                    .transpose()?,
                limit: *limit,
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
                pat,
                source,
                next_index,
                body,
                iterations,
                loop_scope,
                span,
                frame,
            } => {
                let mut loop_scope = loop_scope.iter().copied().collect::<Vec<_>>();
                loop_scope.sort_by_key(|symbol| symbol.0);
                Self::ForLoop {
                    pat: *pat,
                    source: source
                        .as_ref()
                        .map(|source| context.value(source.value()))
                        .transpose()?,
                    next_index: *next_index,
                    body: *body,
                    iterations: *iterations,
                    loop_scope,
                    span: *span,
                    frame: context.frame(frame)?,
                }
            }
            Continuation::WhileLoop {
                cond,
                body,
                iteration,
                max_iterations,
                resume_after_body,
                span,
                frame,
            } => Self::WhileLoop {
                cond: *cond,
                body: *body,
                iteration: *iteration,
                max_iterations: *max_iterations,
                resume_after_body: *resume_after_body,
                span: *span,
                frame: context.frame(frame)?,
            },
            Continuation::RetryAttempt {
                retry,
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame,
            } => Self::RetryAttempt {
                retry: retry.clone(),
                body: *body,
                attempts: *attempts,
                next_attempt: *next_attempt,
                block: *block,
                next_stmt_index: *next_stmt_index,
                frame: context.frame(frame)?,
            },
            Continuation::TryExpr { expr, span } => Self::TryExpr {
                expr: *expr,
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
            | Continuation::HandleBoundary { .. }
            | Continuation::ScopedModelPolicy { .. }
            | Continuation::Chain { .. } => {
                return Err("continuation capture expected a leaf, not an owned edge".into());
            }
            Continuation::Return => Self::Return,
            Continuation::Resume => Self::Resume,
            Continuation::Finish => Self::Finish,
            Continuation::BlockValue => Self::BlockValue,
        })
    }

    fn restore_leaf(self, context: &mut RestoreContext) -> Result<Continuation, String> {
        Ok(match self {
            Self::ProgramResume { id } => Continuation::ProgramResume(context.suspension(id)?),
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
                new_value: new_value.restore_with(context)?,
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
                left: left.restore_with(context)?,
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
                head: head.restore_with(context)?,
                span,
            },
            Self::RangeStart { end, bounds, frame } => Continuation::RangeStart {
                end,
                bounds,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::RangeEnd { start, bounds } => Continuation::RangeEnd {
                start: start.restore_with(context)?,
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
                key: key.restore_with(context)?,
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
                base: base.restore_with(context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SliceBase { eval, frame } => Continuation::SliceBase {
                eval: restore_slice_eval(eval),
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SliceStart { eval, base, frame } => Continuation::SliceStart {
                eval: restore_slice_eval(eval),
                base: base.restore_with(context)?,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SliceEnd {
                eval,
                base,
                start,
                frame,
            } => Continuation::SliceEnd {
                eval: restore_slice_eval(eval),
                base: base.restore_with(context)?,
                start: start.restore_with(context)?,
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
            Self::LocalMethodArgs {
                expr,
                receiver,
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::LocalMethodArgs {
                expr,
                receiver: receiver.restore_with(context)?,
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::StaticMethodArgs {
                expr,
                kind,
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::StaticMethodArgs {
                expr,
                kind: restore_static_method_kind(kind),
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::SpecMethodReceiver {
                expr,
                receiver_expr,
                spec_symbol,
                spec_args,
                method,
                args,
                span,
                frame,
            } => Continuation::SpecMethodReceiver {
                expr,
                receiver_expr,
                spec_symbol,
                spec_args,
                method,
                args,
                span,
                frame: super::frame::restore_frame(frame, context)?,
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
                target: super::call_target::restore_call_target(target, context)?,
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
                action,
                type_args,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MemoryArgs {
                region_stable_id,
                path,
                key_type,
                value_type,

                result_type,
                method,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::MemoryArgs {
                region_stable_id,
                path,
                key_type,
                value_type,

                result_type,
                method,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args, context)?,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::MemorySelectionLimitArgs {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate,
                limit,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::MemorySelectionLimitArgs {
                region_stable_id,
                path,
                key_type,
                value_type,
                kind,
                predicate: predicate
                    .map(|value| value.restore_with(context))
                    .transpose()?,
                limit,
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
                input: input.restore_with(context)?,
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
                pat,
                source,
                next_index,
                body,
                iterations,
                loop_scope,
                span,
                frame,
            } => Continuation::ForLoop {
                pat,
                source: source
                    .map(|source| {
                        crate::value::iteration::IterationSource::new(source.restore_with(context)?)
                    })
                    .transpose()?,
                next_index,
                body,
                iterations,
                loop_scope: loop_scope.into_iter().collect::<HashSet<_>>(),
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::WhileLoop {
                cond,
                body,
                iteration,
                max_iterations,
                resume_after_body,
                span,
                frame,
            } => Continuation::WhileLoop {
                cond,
                body,
                iteration,
                max_iterations,
                resume_after_body,
                span,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::RetryAttempt {
                retry,
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame,
            } => Continuation::RetryAttempt {
                retry,
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame: super::frame::restore_frame(frame, context)?,
            },
            Self::TryExpr { expr, span } => Continuation::TryExpr { expr, span },
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
            | Self::HandleBoundary { .. }
            | Self::ScopedModelPolicy { .. }
            | Self::Chain { .. } => {
                return Err("continuation restore expected a leaf, not an owned edge".into());
            }
            Self::Return => Continuation::Return,
            Self::Resume => Continuation::Resume,
            Self::Finish => Continuation::Finish,
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

fn capture_static_method_kind(value: &StaticMethodKind) -> StaticMethodKindSnapshot {
    match value {
        StaticMethodKind::Prompt => StaticMethodKindSnapshot::Prompt,
        StaticMethodKind::Message => StaticMethodKindSnapshot::Message,
        StaticMethodKind::SessionConfig => StaticMethodKindSnapshot::SessionConfig,
        StaticMethodKind::Conversation => StaticMethodKindSnapshot::Conversation,
        StaticMethodKind::Range => StaticMethodKindSnapshot::Range,
        StaticMethodKind::AdvancedCollection(value) => {
            StaticMethodKindSnapshot::AdvancedCollection(value.clone())
        }
    }
}

fn restore_static_method_kind(value: StaticMethodKindSnapshot) -> StaticMethodKind {
    match value {
        StaticMethodKindSnapshot::Prompt => StaticMethodKind::Prompt,
        StaticMethodKindSnapshot::Message => StaticMethodKind::Message,
        StaticMethodKindSnapshot::SessionConfig => StaticMethodKind::SessionConfig,
        StaticMethodKindSnapshot::Conversation => StaticMethodKind::Conversation,
        StaticMethodKindSnapshot::Range => StaticMethodKind::Range,
        StaticMethodKindSnapshot::AdvancedCollection(value) => {
            StaticMethodKind::AdvancedCollection(value)
        }
    }
}
