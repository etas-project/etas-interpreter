use super::support::*;
use super::*;
use crate::orchestration::HandlerScopeId;

pub(crate) fn continuation_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
    slots: Arc<SlotLayoutTable>,
) -> Result<Continuation, String> {
    let kind = required_str(value, "kind")?;
    Ok(match kind {
        "continue_block" => Continuation::ContinueBlock {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "bind" => Continuation::Bind {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            pat: HirPatId(required_u32(value, "pat")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "assign" => Continuation::Assign {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            target: HirExprId(required_u32(value, "target")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "assign_target_index" => Continuation::AssignTargetIndex {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            root_symbol: SymbolId(required_u32(value, "root_symbol")?),
            segments: local_place_segments_from_snapshot(limits, required(value, "segments")?)?,
            components: local_place_components_from_snapshot(required(value, "components")?)?,
            next_component_index: required_usize(value, "next_component_index")?,
            new_value: value_from_json_with_limits(limits, required(value, "new_value")?)
                .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "field_receiver" => Continuation::FieldReceiver {
            expr: HirExprId(required_u32(value, "expr")?),
            field: required_str(value, "field")?.to_owned(),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "unary" => Continuation::Unary {
            op: unary_op_from_name(required_str(value, "op")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "binary_left" => Continuation::BinaryLeft {
            op: binary_op_from_name(required_str(value, "op")?)?,
            rhs: HirExprId(required_u32(value, "rhs")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "binary_right" => Continuation::BinaryRight {
            op: binary_op_from_name(required_str(value, "op")?)?,
            left: value_from_json_with_limits(limits, required(value, "left")?)
                .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "aggregate_element" => Continuation::AggregateElement {
            kind: aggregate_kind_from_name(required_str(value, "aggregate_kind")?)?,
            exprs: required_u32_array(value, "exprs")?
                .into_iter()
                .map(HirExprId)
                .collect(),
            next_index: required_usize(value, "next_index")?,
            values: required_values(limits, value, "values")?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "list_cons_head" => Continuation::ListConsHead {
            tail: HirExprId(required_u32(value, "tail")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "list_cons_tail" => Continuation::ListConsTail {
            head: value_from_json_with_limits(limits, required(value, "head")?)
                .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "range_start" => Continuation::RangeStart {
            end: HirExprId(required_u32(value, "end")?),
            bounds: range_bounds_from_name(required_str(value, "bounds")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "range_end" => Continuation::RangeEnd {
            start: value_from_json_with_limits(limits, required(value, "start")?)
                .map_err(|error| error.to_string())?,
            bounds: range_bounds_from_name(required_str(value, "bounds")?)?,
        },
        "record_field" => Continuation::RecordField {
            nominal_type: optional_u32(value, "nominal_type")?.map(etas_types::TypeId),
            fields: required(value, "fields")?
                .as_array()
                .ok_or_else(|| "machine snapshot `fields` must be an array".to_owned())?
                .iter()
                .map(field_init_from_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
            next_index: required_usize(value, "next_index")?,
            values: record_values_from_snapshot(limits, required(value, "values")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "map_key" => Continuation::MapKey {
            entries: map_entries_from_snapshot(required(value, "entries")?)?,
            index: required_usize(value, "index")?,
            values: map_values_from_snapshot(limits, required(value, "values")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "map_value" => Continuation::MapValue {
            entries: map_entries_from_snapshot(required(value, "entries")?)?,
            index: required_usize(value, "index")?,
            key: value_from_json_with_limits(limits, required(value, "key")?)
                .map_err(|error| error.to_string())?,
            values: map_values_from_snapshot(limits, required(value, "values")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "index_base" => Continuation::IndexBase {
            expr: HirExprId(required_u32(value, "expr")?),
            index: HirExprId(required_u32(value, "index")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "index_value" => Continuation::IndexValue {
            expr: HirExprId(required_u32(value, "expr")?),
            base: value_from_json_with_limits(limits, required(value, "base")?)
                .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "slice_base" => Continuation::SliceBase {
            eval: slice_eval_from_snapshot(required(value, "eval")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "slice_start" => Continuation::SliceStart {
            eval: slice_eval_from_snapshot(required(value, "eval")?)?,
            base: value_from_json_with_limits(limits, required(value, "base")?)
                .map_err(|error| error.to_string())?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "slice_end" => Continuation::SliceEnd {
            eval: slice_eval_from_snapshot(required(value, "eval")?)?,
            base: value_from_json_with_limits(limits, required(value, "base")?)
                .map_err(|error| error.to_string())?,
            start: value_from_json_with_limits(limits, required(value, "start")?)
                .map_err(|error| error.to_string())?,
        },
        "method_receiver" => Continuation::MethodReceiver {
            expr: HirExprId(required_u32(value, "expr")?),
            method: required_str(value, "method")?.to_owned(),
            type_args: required_u32_array(value, "type_args")?
                .into_iter()
                .map(HirTypeId)
                .collect(),
            args: args_from_snapshot(required(value, "args")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "prompt_value_method_arg" => Continuation::PromptValueMethodArg {
            messages: prompt_messages_from_snapshot(required(value, "messages")?)?,
            method: required_str(value, "method")?.to_owned(),
            role: prompt_role_from_name(required_str(value, "role")?)?,
            allow_plain_system_content: required_bool(value, "allow_plain_system_content")?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "local_method_args" => Continuation::LocalMethodArgs {
            expr: HirExprId(required_u32(value, "expr")?),
            receiver: value_from_json_with_limits(limits, required(value, "receiver")?)
                .map_err(|error| error.to_string())?,
            method: required_str(value, "method")?.to_owned(),
            type_args: required_u32_array(value, "type_args")?
                .into_iter()
                .map(HirTypeId)
                .collect(),
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "static_method_args" => Continuation::StaticMethodArgs {
            expr: HirExprId(required_u32(value, "expr")?),
            kind: static_method_kind_from_snapshot(required(value, "static_kind")?)?,
            method: required_str(value, "method")?.to_owned(),
            type_args: required_u32_array(value, "type_args")?
                .into_iter()
                .map(HirTypeId)
                .collect(),
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "spec_method_receiver" => Continuation::SpecMethodReceiver {
            expr: HirExprId(required_u32(value, "expr")?),
            receiver_expr: HirExprId(required_u32(value, "receiver_expr")?),
            spec_symbol: SymbolId(required_u32(value, "spec_symbol")?),
            spec_args: required_u32_array(value, "spec_args")?
                .into_iter()
                .map(HirTypeId)
                .collect(),
            method: required_str(value, "method")?.to_owned(),
            args: args_from_snapshot(required(value, "args")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "callee_eval" => Continuation::CalleeEval {
            args: args_from_snapshot(required(value, "args")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "pipeline_stage_target" => Continuation::PipelineStageTarget {
            stages: stages_from_snapshot(required(value, "stages")?)?,
            next_stage_index: required_usize(value, "next_stage_index")?,
            targets: call_targets_from_snapshot(
                limits,
                required(value, "targets")?,
                slots.clone(),
            )?,
            current_limits: runtime_limits_from_snapshot(required(value, "current_limits")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "call_args" => Continuation::CallArgs {
            target: call_target_from_snapshot(limits, required(value, "target")?, slots.clone())?,
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "variant_args" => Continuation::VariantArgs {
            variant_symbol: SymbolId(required_u32(value, "variant_symbol")?),
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "perform_args" => {
            let expr = HirExprId(required_u32(value, "expr")?);
            let Some(etas_hir::HirExpr::Perform {
                action, args, span, ..
            }) = checked.hir.exprs.get(expr)
            else {
                return Err(format!(
                    "machine perform continuation expression {} is not a perform node",
                    expr.0
                ));
            };
            Continuation::PerformArgs {
                expr,
                action: action.clone(),
                type_args: required_u32_array(value, "type_args")?
                    .into_iter()
                    .map(HirTypeId)
                    .collect(),
                args: args.clone(),
                next_arg_index: required_usize(value, "next_arg_index")?,
                evaluated_args: required_values(limits, value, "evaluated_args")?,
                span: *span,
                frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
            }
        }
        "memory_args" => Continuation::MemoryArgs {
            result_type: etas_types::TypeId(required_u32(value, "result_type")?),
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: required_string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
            method: required_str(value, "method")?.to_owned(),
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "memory_selection_limit_args" => Continuation::MemorySelectionLimitArgs {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: required_string_array(value, "path")?,
            key_type: etas_types::TypeId(required_u32(value, "key_type")?),
            value_type: etas_types::TypeId(required_u32(value, "value_type")?),
            kind: crate::value::codec::memory_selection_kind_from_json(required_str(
                value,
                "selection_kind",
            )?)?,
            predicate: optional_value(limits, value, "predicate")?,
            limit: optional_u32(value, "limit")?,
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "if_expr" => Continuation::IfExpr {
            then_block: HirBlockId(required_u32(value, "then_block")?),
            else_branch: optional_else_branch(value, "else_branch")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "if_stmt" => Continuation::IfStmt {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            then_block: HirBlockId(required_u32(value, "then_block")?),
            else_branch: optional_else_branch(value, "else_branch")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "match_expr" => Continuation::MatchExpr {
            arms: match_arms_from_snapshot(required(value, "arms")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "match_stmt" => Continuation::MatchStmt {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            arms: match_arms_from_snapshot(required(value, "arms")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "handle_handler" => Continuation::HandleHandler {
            handle_expr: HirExprId(required_u32(value, "handle_expr")?),
            body: HirExprId(required_u32(value, "body")?),
            handler: HirExprId(required_u32(value, "handler")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "pipeline_input" => Continuation::PipelineInput {
            stages: stages_from_snapshot(required(value, "stages")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "pipeline_target" => Continuation::PipelineTarget {
            input: value_from_json_with_limits(limits, required(value, "input")?)
                .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "composed_call" => Continuation::ComposedCall {
            remaining: call_targets_from_snapshot(limits, required(value, "remaining")?, slots)?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "restore_model_policy" => Continuation::RestoreModelPolicy {
            previous: Box::new(model_policy_from_snapshot(required(value, "previous")?)?),
            inner: Box::new(continuation_from_snapshot(
                limits,
                required(value, "inner")?,
                checked,
                slots,
            )?),
        },
        "try_expr" => Continuation::TryExpr {
            expr: HirExprId(required_u32(value, "expr")?),
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "memory_clear_delete_all" => Continuation::MemoryClearDeleteAll {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: required_string_array(value, "path")?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "memory_clear_delete_next" => Continuation::MemoryClearDeleteNext {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: required_string_array(value, "path")?,
            remaining_keys: required_values(limits, value, "remaining_keys")?,
            next_index: required_usize(value, "next_index")?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "for_loop" => Continuation::ForLoop {
            pat: HirPatId(required_u32(value, "pat")?),
            values: optional_values(limits, value, "values")?,
            next_index: required_usize(value, "next_index")?,
            body: HirBlockId(required_u32(value, "body")?),
            iterations: required_usize(value, "iterations")?,
            loop_scope: required_u32_array(value, "loop_scope")?
                .into_iter()
                .map(SymbolId)
                .collect(),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "while_loop" => Continuation::WhileLoop {
            cond: HirExprId(required_u32(value, "cond")?),
            body: HirBlockId(required_u32(value, "body")?),
            iteration: required_u32(value, "iteration")?,
            max_iterations: required_u32(value, "max_iterations")?,
            resume_after_body: required_bool(value, "resume_after_body")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "retry_attempt" => Continuation::RetryAttempt {
            retry: RetryAttemptRecord {
                id: RetryAttemptId(required_u32(value, "retry_id")?),
                ordinal: required_u32(value, "retry_ordinal")?,
            },
            body: HirBlockId(required_u32(value, "body")?),
            attempts: required_usize(value, "attempts")?,
            next_attempt: required_usize(value, "next_attempt")?,
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "handle_boundary" => Continuation::HandleBoundary {
            scope_id: HandlerScopeId(required_u32(value, "scope_id")?),
            inner: Box::new(continuation_from_snapshot(
                limits,
                required(value, "inner")?,
                checked,
                slots.clone(),
            )?),
            handlers: required(value, "handlers")?
                .as_array()
                .ok_or_else(|| "machine snapshot `handlers` must be an array".to_owned())?
                .iter()
                .map(handler_arm_from_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: frame_from_snapshot(limits, required(value, "frame")?, slots)?,
        },
        "handler_dispatch" => Continuation::HandlerDispatch {
            outer: Box::new(continuation_from_snapshot(
                limits,
                required(value, "outer")?,
                checked,
                slots,
            )?),
        },
        "agent_prompt_body" => Continuation::AgentPromptBody {
            item: etas_hir::HirItemId(required_u32(value, "item")?),
            span: span_from_snapshot(required(value, "span")?)?,
            model_policy: optional_model_policy(value, "model_policy")?.map(Box::new),
        },
        "scoped_model_policy" => Continuation::ScopedModelPolicy {
            policy: Box::new(model_policy_from_snapshot(required(value, "policy")?)?),
            inner: Box::new(continuation_from_snapshot(
                limits,
                required(value, "inner")?,
                checked,
                slots,
            )?),
        },
        "call_boundary" => Continuation::CallBoundary {
            outer: Box::new(continuation_from_snapshot(
                limits,
                required(value, "outer")?,
                checked,
                slots,
            )?),
        },
        "chain" => Continuation::Chain {
            inner: Box::new(continuation_from_snapshot(
                limits,
                required(value, "inner")?,
                checked,
                slots.clone(),
            )?),
            outer: Box::new(continuation_from_snapshot(
                limits,
                required(value, "outer")?,
                checked,
                slots,
            )?),
        },
        "return" => Continuation::Return,
        "resume" => Continuation::Resume,
        "finish" => Continuation::Finish,
        "block_value" => Continuation::BlockValue,
        other => return Err(format!("unknown machine continuation `{other}`")),
    })
}
