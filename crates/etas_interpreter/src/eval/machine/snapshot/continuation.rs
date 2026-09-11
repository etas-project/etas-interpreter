use std::collections::HashSet;

use crate::api::{ModelExecutionPolicy, ModelResponseDecodePolicy};
use crate::control::{AggregateKind, Continuation, StaticMethodKind};
use crate::eval::{LocalPlaceComponent, LocalPlaceSegment, SliceExprEval};
use crate::orchestration::{
    AggregateKindSnapshot, ContinuationSnapshot, LocalPlaceComponentSnapshot,
    LocalPlaceSegmentSnapshot, ModelExecutionPolicySnapshot, ModelResponseDecodeSnapshot,
    SliceExprEvalSnapshot, StaticMethodKindSnapshot, ValueSnapshot,
};

impl ContinuationSnapshot {
    pub(crate) fn capture(continuation: &Continuation) -> Result<Self, String> {
        Ok(match continuation {
            Continuation::ContinueBlock {
                block,
                next_stmt_index,
                frame,
            } => Self::ContinueBlock {
                block: *block,
                next_stmt_index: *next_stmt_index,
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                    .map(capture_local_segment)
                    .collect::<Result<Vec<_>, _>>()?,
                components: components.iter().map(capture_local_component).collect(),
                next_component_index: *next_component_index,
                new_value: ValueSnapshot::capture(new_value)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::BinaryRight { op, left, span } => Self::BinaryRight {
                op: *op,
                left: ValueSnapshot::capture(left)?,
                span: *span,
            },
            Continuation::AggregateElement {
                kind,
                exprs,
                next_index,
                values,
                frame,
            } => Self::AggregateElement {
                kind: capture_aggregate_kind(*kind),
                exprs: exprs.clone(),
                next_index: *next_index,
                values: capture_values(values)?,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::ListConsHead { tail, span, frame } => Self::ListConsHead {
                tail: *tail,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::ListConsTail { head, span } => Self::ListConsTail {
                head: ValueSnapshot::capture(head)?,
                span: *span,
            },
            Continuation::RangeStart { end, bounds, frame } => Self::RangeStart {
                end: *end,
                bounds: *bounds,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::RangeEnd { start, bounds } => Self::RangeEnd {
                start: ValueSnapshot::capture(start)?,
                bounds: *bounds,
            },
            Continuation::RecordField {
                nominal_type,
                variant_symbol,
                fields,
                next_index,
                values,
                frame,
            } => Self::RecordField {
                nominal_type: *nominal_type,
                variant_symbol: *variant_symbol,
                fields: fields.clone(),
                next_index: *next_index,
                values: values
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), ValueSnapshot::capture(value)?)))
                    .collect::<Result<Vec<_>, String>>()?,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::MapKey {
                entries,
                index,
                values,
                frame,
            } => Self::MapKey {
                entries: entries.clone(),
                index: *index,
                values: capture_pairs(values)?,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::MapValue {
                entries,
                index,
                key,
                values,
                frame,
            } => Self::MapValue {
                entries: entries.clone(),
                index: *index,
                key: ValueSnapshot::capture(key)?,
                values: capture_pairs(values)?,
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::IndexValue { expr, base, span } => Self::IndexValue {
                expr: *expr,
                base: ValueSnapshot::capture(base)?,
                span: *span,
            },
            Continuation::SliceBase { eval, frame } => Self::SliceBase {
                eval: capture_slice_eval(*eval),
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::SliceStart { eval, base, frame } => Self::SliceStart {
                eval: capture_slice_eval(*eval),
                base: ValueSnapshot::capture(base)?,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::SliceEnd { eval, base, start } => Self::SliceEnd {
                eval: capture_slice_eval(*eval),
                base: ValueSnapshot::capture(base)?,
                start: ValueSnapshot::capture(start)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                receiver: ValueSnapshot::capture(receiver)?,
                method: method.clone(),
                type_args: type_args.clone(),
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::CalleeEval { args, span, frame } => Self::CalleeEval {
                args: args.clone(),
                span: *span,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::PipelineStageTarget {
                stages,
                next_stage_index,
                targets,
                current_limits,
                span,
                frame,
            } => Self::PipelineStageTarget {
                stages: stages.clone(),
                next_stage_index: *next_stage_index,
                targets: targets
                    .iter()
                    .map(super::call_target::capture_call_target)
                    .collect::<Result<Vec<_>, _>>()?,
                current_limits: current_limits.clone(),
                span: *span,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::CallArgs {
                target,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Self::CallArgs {
                target: super::call_target::capture_call_target(target)?,
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                predicate: predicate.as_ref().map(ValueSnapshot::capture).transpose()?,
                limit: *limit,
                args: args.clone(),
                next_arg_index: *next_arg_index,
                evaluated_args: capture_values(evaluated_args)?,
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::MatchExpr { arms, span, frame } => Self::MatchExpr {
                arms: arms.clone(),
                span: *span,
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::PipelineInput {
                stages,
                span,
                frame,
            } => Self::PipelineInput {
                stages: stages.clone(),
                span: *span,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::PipelineTarget { input, span } => Self::PipelineTarget {
                input: ValueSnapshot::capture(input)?,
                span: *span,
            },
            Continuation::ComposedCall { remaining, span } => Self::ComposedCall {
                remaining: remaining
                    .iter()
                    .map(super::call_target::capture_call_target)
                    .collect::<Result<Vec<_>, _>>()?,
                span: *span,
            },
            Continuation::RestoreModelPolicy { previous, inner } => Self::RestoreModelPolicy {
                previous: Box::new(capture_model_policy(previous)),
                inner: Box::new(Self::capture(inner)?),
            },
            Continuation::CallBoundary { outer } => Self::CallBoundary {
                outer: Box::new(Self::capture(outer)?),
            },
            Continuation::ForLoop {
                pat,
                values,
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
                    values: values
                        .as_ref()
                        .map(|values| capture_values(values))
                        .transpose()?,
                    next_index: *next_index,
                    body: *body,
                    iterations: *iterations,
                    loop_scope,
                    span: *span,
                    frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                frame: super::frame::capture_frame(frame)?,
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
                remaining_keys: capture_values(remaining_keys)?,
                next_index: *next_index,
                span: *span,
            },
            Continuation::HandlerDispatch { outer } => Self::HandlerDispatch {
                outer: Box::new(Self::capture(outer)?),
            },
            Continuation::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span,
                frame,
            } => Self::HandleBoundary {
                scope_id: *scope_id,
                inner: Box::new(Self::capture(inner)?),
                handlers: handlers.clone(),
                span: *span,
                frame: super::frame::capture_frame(frame)?,
            },
            Continuation::AgentPromptBody {
                item,
                span,
                model_policy,
            } => Self::AgentPromptBody {
                item: *item,
                span: *span,
                model_policy: model_policy
                    .as_deref()
                    .map(capture_model_policy)
                    .map(Box::new),
            },
            Continuation::ScopedModelPolicy { policy, inner } => Self::ScopedModelPolicy {
                policy: Box::new(capture_model_policy(policy)),
                inner: Box::new(Self::capture(inner)?),
            },
            Continuation::Chain { inner, outer } => Self::Chain {
                inner: Box::new(Self::capture(inner)?),
                outer: Box::new(Self::capture(outer)?),
            },
            Continuation::Return => Self::Return,
            Continuation::Resume => Self::Resume,
            Continuation::Finish => Self::Finish,
            Continuation::BlockValue => Self::BlockValue,
        })
    }

    pub(crate) fn restore(self) -> Result<Continuation, String> {
        Ok(match self {
            Self::ContinueBlock {
                block,
                next_stmt_index,
                frame,
            } => Continuation::ContinueBlock {
                block,
                next_stmt_index,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                    .map(restore_local_segment)
                    .collect::<Result<Vec<_>, _>>()?,
                components: components
                    .into_iter()
                    .map(restore_local_component)
                    .collect(),
                next_component_index,
                new_value: new_value.restore()?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
            },
            Self::BinaryRight { op, left, span } => Continuation::BinaryRight {
                op,
                left: left.restore()?,
                span,
            },
            Self::AggregateElement {
                kind,
                exprs,
                next_index,
                values,
                frame,
            } => Continuation::AggregateElement {
                kind: restore_aggregate_kind(kind),
                exprs,
                next_index,
                values: restore_values(values)?,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::ListConsHead { tail, span, frame } => Continuation::ListConsHead {
                tail,
                span,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::ListConsTail { head, span } => Continuation::ListConsTail {
                head: head.restore()?,
                span,
            },
            Self::RangeStart { end, bounds, frame } => Continuation::RangeStart {
                end,
                bounds,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::RangeEnd { start, bounds } => Continuation::RangeEnd {
                start: start.restore()?,
                bounds,
            },
            Self::RecordField {
                nominal_type,
                variant_symbol,
                fields,
                next_index,
                values,
                frame,
            } => Continuation::RecordField {
                nominal_type,
                variant_symbol,
                fields,
                next_index,
                values: values
                    .into_iter()
                    .map(|(name, value)| Ok((name, value.restore()?)))
                    .collect::<Result<Vec<_>, String>>()?,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::MapKey {
                entries,
                index,
                values,
                frame,
            } => Continuation::MapKey {
                entries,
                index,
                values: restore_pairs(values)?,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::MapValue {
                entries,
                index,
                key,
                values,
                frame,
            } => Continuation::MapValue {
                entries,
                index,
                key: key.restore()?,
                values: restore_pairs(values)?,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
            },
            Self::IndexValue { expr, base, span } => Continuation::IndexValue {
                expr,
                base: base.restore()?,
                span,
            },
            Self::SliceBase { eval, frame } => Continuation::SliceBase {
                eval: restore_slice_eval(eval),
                frame: super::frame::restore_frame(frame)?,
            },
            Self::SliceStart { eval, base, frame } => Continuation::SliceStart {
                eval: restore_slice_eval(eval),
                base: base.restore()?,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::SliceEnd { eval, base, start } => Continuation::SliceEnd {
                eval: restore_slice_eval(eval),
                base: base.restore()?,
                start: start.restore()?,
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
                frame: super::frame::restore_frame(frame)?,
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
                receiver: receiver.restore()?,
                method,
                type_args,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
            },
            Self::CalleeEval { args, span, frame } => Continuation::CalleeEval {
                args,
                span,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::PipelineStageTarget {
                stages,
                next_stage_index,
                targets,
                current_limits,
                span,
                frame,
            } => Continuation::PipelineStageTarget {
                stages,
                next_stage_index,
                targets: targets
                    .into_iter()
                    .map(super::call_target::restore_call_target)
                    .collect::<Result<Vec<_>, _>>()?,
                current_limits,
                span,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::CallArgs {
                target,
                args,
                next_arg_index,
                evaluated_args,
                span,
                frame,
            } => Continuation::CallArgs {
                target: super::call_target::restore_call_target(target)?,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                predicate: predicate.map(ValueSnapshot::restore).transpose()?,
                limit,
                args,
                next_arg_index,
                evaluated_args: restore_values(evaluated_args)?,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
            },
            Self::MatchExpr { arms, span, frame } => Continuation::MatchExpr {
                arms,
                span,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
            },
            Self::PipelineInput {
                stages,
                span,
                frame,
            } => Continuation::PipelineInput {
                stages,
                span,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::PipelineTarget { input, span } => Continuation::PipelineTarget {
                input: input.restore()?,
                span,
            },
            Self::ComposedCall { remaining, span } => Continuation::ComposedCall {
                remaining: remaining
                    .into_iter()
                    .map(super::call_target::restore_call_target)
                    .collect::<Result<Vec<_>, _>>()?,
                span,
            },
            Self::RestoreModelPolicy { previous, inner } => Continuation::RestoreModelPolicy {
                previous: Box::new(restore_model_policy(*previous)),
                inner: Box::new(inner.restore()?),
            },
            Self::CallBoundary { outer } => Continuation::CallBoundary {
                outer: Box::new(outer.restore()?),
            },
            Self::ForLoop {
                pat,
                values,
                next_index,
                body,
                iterations,
                loop_scope,
                span,
                frame,
            } => Continuation::ForLoop {
                pat,
                values: values.map(restore_values).transpose()?,
                next_index,
                body,
                iterations,
                loop_scope: loop_scope.into_iter().collect::<HashSet<_>>(),
                span,
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                frame: super::frame::restore_frame(frame)?,
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
                remaining_keys: restore_values(remaining_keys)?,
                next_index,
                span,
            },
            Self::HandlerDispatch { outer } => Continuation::HandlerDispatch {
                outer: Box::new(outer.restore()?),
            },
            Self::HandleBoundary {
                scope_id,
                inner,
                handlers,
                span,
                frame,
            } => Continuation::HandleBoundary {
                scope_id,
                inner: Box::new(inner.restore()?),
                handlers,
                span,
                frame: super::frame::restore_frame(frame)?,
            },
            Self::AgentPromptBody {
                item,
                span,
                model_policy,
            } => Continuation::AgentPromptBody {
                item,
                span,
                model_policy: model_policy
                    .map(|policy| restore_model_policy(*policy))
                    .map(Box::new),
            },
            Self::ScopedModelPolicy { policy, inner } => Continuation::ScopedModelPolicy {
                policy: Box::new(restore_model_policy(*policy)),
                inner: Box::new(inner.restore()?),
            },
            Self::Chain { inner, outer } => Continuation::Chain {
                inner: Box::new(inner.restore()?),
                outer: Box::new(outer.restore()?),
            },
            Self::Return => Continuation::Return,
            Self::Resume => Continuation::Resume,
            Self::Finish => Continuation::Finish,
            Self::BlockValue => Continuation::BlockValue,
        })
    }

    pub(crate) fn to_runtime(&self) -> Result<Continuation, String> {
        self.clone().restore()
    }
}

fn capture_values(values: &[crate::value::InterpValue]) -> Result<Vec<ValueSnapshot>, String> {
    values.iter().map(ValueSnapshot::capture).collect()
}

fn restore_values(values: Vec<ValueSnapshot>) -> Result<Vec<crate::value::InterpValue>, String> {
    values.into_iter().map(ValueSnapshot::restore).collect()
}

fn capture_pairs(
    values: &[(crate::value::InterpValue, crate::value::InterpValue)],
) -> Result<Vec<(ValueSnapshot, ValueSnapshot)>, String> {
    values
        .iter()
        .map(|(key, value)| Ok((ValueSnapshot::capture(key)?, ValueSnapshot::capture(value)?)))
        .collect()
}

fn restore_pairs(
    values: Vec<(ValueSnapshot, ValueSnapshot)>,
) -> Result<Vec<(crate::value::InterpValue, crate::value::InterpValue)>, String> {
    values
        .into_iter()
        .map(|(key, value)| Ok((key.restore()?, value.restore()?)))
        .collect()
}

fn capture_local_segment(value: &LocalPlaceSegment) -> Result<LocalPlaceSegmentSnapshot, String> {
    Ok(match value {
        LocalPlaceSegment::Field(value) => LocalPlaceSegmentSnapshot::Field(value.clone()),
        LocalPlaceSegment::Index(value) => LocalPlaceSegmentSnapshot::Index(*value),
        LocalPlaceSegment::MapKey(value) => {
            LocalPlaceSegmentSnapshot::MapKey(Box::new(ValueSnapshot::capture(value)?))
        }
    })
}

fn restore_local_segment(value: LocalPlaceSegmentSnapshot) -> Result<LocalPlaceSegment, String> {
    Ok(match value {
        LocalPlaceSegmentSnapshot::Field(value) => LocalPlaceSegment::Field(value),
        LocalPlaceSegmentSnapshot::Index(value) => LocalPlaceSegment::Index(value),
        LocalPlaceSegmentSnapshot::MapKey(value) => {
            LocalPlaceSegment::MapKey(Box::new(value.restore()?))
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

fn capture_aggregate_kind(value: AggregateKind) -> AggregateKindSnapshot {
    match value {
        AggregateKind::Tuple => AggregateKindSnapshot::Tuple,
        AggregateKind::Array => AggregateKindSnapshot::Array,
        AggregateKind::List => AggregateKindSnapshot::List,
        AggregateKind::Set => AggregateKindSnapshot::Set,
    }
}

fn restore_aggregate_kind(value: AggregateKindSnapshot) -> AggregateKind {
    match value {
        AggregateKindSnapshot::Tuple => AggregateKind::Tuple,
        AggregateKindSnapshot::Array => AggregateKind::Array,
        AggregateKindSnapshot::List => AggregateKind::List,
        AggregateKindSnapshot::Set => AggregateKind::Set,
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

fn capture_model_policy(policy: &ModelExecutionPolicy) -> ModelExecutionPolicySnapshot {
    ModelExecutionPolicySnapshot {
        provider: policy.provider.clone(),
        provider_capabilities: policy.provider_capabilities,
        model: policy.model.clone(),
        model_locked: policy.model_locked,
        tools: policy.tools.clone(),
        tool_choice: policy.tool_choice.clone(),
        policy_ref: policy.policy_ref.clone(),
        options: policy.options.clone(),
        budget: policy.budget.clone(),
        response_decode: match policy.response_decode {
            ModelResponseDecodePolicy::String => ModelResponseDecodeSnapshot::String,
            ModelResponseDecodePolicy::ModelResponse => ModelResponseDecodeSnapshot::ModelResponse,
        },
        max_tool_rounds: policy.max_tool_rounds,
    }
}

fn restore_model_policy(policy: ModelExecutionPolicySnapshot) -> ModelExecutionPolicy {
    ModelExecutionPolicy {
        provider: policy.provider,
        provider_capabilities: policy.provider_capabilities,
        model: policy.model,
        model_locked: policy.model_locked,
        tools: policy.tools,
        tool_choice: policy.tool_choice,
        policy_ref: policy.policy_ref,
        options: policy.options,
        budget: policy.budget,
        response_decode: match policy.response_decode {
            ModelResponseDecodeSnapshot::String => ModelResponseDecodePolicy::String,
            ModelResponseDecodeSnapshot::ModelResponse => ModelResponseDecodePolicy::ModelResponse,
        },
        max_tool_rounds: policy.max_tool_rounds,
    }
}
