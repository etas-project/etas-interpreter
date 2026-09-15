use super::support::*;
use super::*;
mod traversal;
pub(crate) use traversal::continuation_from_snapshot;

fn continuation_leaf_from_snapshot(
    limits: &etas_host::StorageLimits,
    value: &Value,
    checked: &etas_frontend::CheckedProject,
    kind: &str,
) -> Result<ContinuationSnapshot, String> {
    Ok(match kind {
        "continue_block" => ContinuationSnapshot::ContinueBlock {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "bind" => ContinuationSnapshot::Bind {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            pat: HirPatId(required_u32(value, "pat")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "assign" => ContinuationSnapshot::Assign {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            target: HirExprId(required_u32(value, "target")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "assign_target_index" => ContinuationSnapshot::AssignTargetIndex {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            root_symbol: SymbolId(required_u32(value, "root_symbol")?),
            segments: local_place_segments_from_snapshot(limits, required(value, "segments")?)?,
            components: local_place_components_from_snapshot(required(value, "components")?)?,
            next_component_index: required_usize(value, "next_component_index")?,
            new_value: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "new_value")?,
            )
            .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "field_receiver" => ContinuationSnapshot::FieldReceiver {
            expr: HirExprId(required_u32(value, "expr")?),
            field: required_str(value, "field")?.to_owned(),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "unary" => ContinuationSnapshot::Unary {
            op: unary_op_from_name(required_str(value, "op")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "binary_left" => ContinuationSnapshot::BinaryLeft {
            op: binary_op_from_name(required_str(value, "op")?)?,
            rhs: HirExprId(required_u32(value, "rhs")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "binary_right" => ContinuationSnapshot::BinaryRight {
            op: binary_op_from_name(required_str(value, "op")?)?,
            left: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "left")?,
            )
            .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "aggregate_element" => ContinuationSnapshot::AggregateElement {
            expr: HirExprId(required_u32(value, "expr")?),
            next_index: required_usize(value, "next_index")?,
            values: required_values(limits, value, "values")?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "list_cons_head" => ContinuationSnapshot::ListConsHead {
            tail: HirExprId(required_u32(value, "tail")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "list_cons_tail" => ContinuationSnapshot::ListConsTail {
            head: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "head")?,
            )
            .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "range_start" => ContinuationSnapshot::RangeStart {
            end: HirExprId(required_u32(value, "end")?),
            bounds: range_bounds_from_name(required_str(value, "bounds")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "range_end" => ContinuationSnapshot::RangeEnd {
            start: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "start")?,
            )
            .map_err(|error| error.to_string())?,
            bounds: range_bounds_from_name(required_str(value, "bounds")?)?,
        },
        "record_field" => ContinuationSnapshot::RecordField {
            expr: HirExprId(required_u32(value, "expr")?),
            nominal_type: optional_u32(value, "nominal_type")?.map(etas_types::TypeId),
            variant_symbol: optional_u32(value, "variant_symbol")?.map(etas_hir::SymbolId),
            next_index: required_usize(value, "next_index")?,
            values: record_values_from_snapshot(limits, required(value, "values")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "map_key" => ContinuationSnapshot::MapKey {
            expr: HirExprId(required_u32(value, "expr")?),
            index: required_usize(value, "index")?,
            values: map_values_from_snapshot(limits, required(value, "values")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "map_value" => ContinuationSnapshot::MapValue {
            expr: HirExprId(required_u32(value, "expr")?),
            index: required_usize(value, "index")?,
            key: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "key")?,
            )
            .map_err(|error| error.to_string())?,
            values: map_values_from_snapshot(limits, required(value, "values")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "index_base" => ContinuationSnapshot::IndexBase {
            expr: HirExprId(required_u32(value, "expr")?),
            index: HirExprId(required_u32(value, "index")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "index_value" => ContinuationSnapshot::IndexValue {
            expr: HirExprId(required_u32(value, "expr")?),
            base: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "base")?,
            )
            .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "slice_base" => ContinuationSnapshot::SliceBase {
            eval: slice_eval_from_snapshot(required(value, "eval")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "slice_start" => ContinuationSnapshot::SliceStart {
            eval: slice_eval_from_snapshot(required(value, "eval")?)?,
            base: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "base")?,
            )
            .map_err(|error| error.to_string())?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "slice_end" => ContinuationSnapshot::SliceEnd {
            eval: slice_eval_from_snapshot(required(value, "eval")?)?,
            base: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "base")?,
            )
            .map_err(|error| error.to_string())?,
            start: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "start")?,
            )
            .map_err(|error| error.to_string())?,
        },
        "method_receiver" => ContinuationSnapshot::MethodReceiver {
            expr: HirExprId(required_u32(value, "expr")?),
            method: required_str(value, "method")?.to_owned(),
            type_args: required_u32_array(value, "type_args")?
                .into_iter()
                .map(HirTypeId)
                .collect(),
            args: args_from_snapshot(required(value, "args")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "prompt_value_method_arg" => ContinuationSnapshot::PromptValueMethodArg {
            messages: prompt_messages_from_snapshot(required(value, "messages")?)?,
            method: required_str(value, "method")?.to_owned(),
            role: prompt_role_from_name(required_str(value, "role")?)?,
            allow_plain_system_content: required_bool(value, "allow_plain_system_content")?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "local_method_args" => ContinuationSnapshot::LocalMethodArgs {
            expr: HirExprId(required_u32(value, "expr")?),
            receiver: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "receiver")?,
            )
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
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "static_method_args" => ContinuationSnapshot::StaticMethodArgs {
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
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "spec_method_receiver" => ContinuationSnapshot::SpecMethodReceiver {
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
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "callee_eval" => ContinuationSnapshot::CalleeEval {
            args: args_from_snapshot(required(value, "args")?)?.into(),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "pipeline_stage_target" => ContinuationSnapshot::PipelineStageTarget {
            stages: stages_from_snapshot(required(value, "stages")?)?,
            next_stage_index: required_usize(value, "next_stage_index")?,
            targets: call_target_snapshots_from_json(limits, required(value, "targets")?)?,
            current_limits: runtime_limits_from_snapshot(required(value, "current_limits")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "call_args" => ContinuationSnapshot::CallArgs {
            target: call_target_snapshot_from_json(limits, required(value, "target")?)?,
            args: args_from_snapshot(required(value, "args")?)?.into(),
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "variant_args" => ContinuationSnapshot::VariantArgs {
            variant_symbol: SymbolId(required_u32(value, "variant_symbol")?),
            args: args_from_snapshot(required(value, "args")?)?,
            next_arg_index: required_usize(value, "next_arg_index")?,
            evaluated_args: required_values(limits, value, "evaluated_args")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
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
            ContinuationSnapshot::PerformArgs {
                expr,
                action: action.clone(),
                type_args: required_u32_array(value, "type_args")?
                    .into_iter()
                    .map(HirTypeId)
                    .collect(),
                args: std::sync::Arc::from(args.as_slice()),
                next_arg_index: required_usize(value, "next_arg_index")?,
                evaluated_args: required_values(limits, value, "evaluated_args")?,
                span: *span,
                frame: locals_from_snapshot(limits, required(value, "frame")?)?,
            }
        }
        "memory_args" => ContinuationSnapshot::MemoryArgs {
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
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "memory_selection_limit_args" => ContinuationSnapshot::MemorySelectionLimitArgs {
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
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "if_expr" => ContinuationSnapshot::IfExpr {
            then_block: HirBlockId(required_u32(value, "then_block")?),
            else_branch: optional_else_branch(value, "else_branch")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "if_stmt" => ContinuationSnapshot::IfStmt {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            then_block: HirBlockId(required_u32(value, "then_block")?),
            else_branch: optional_else_branch(value, "else_branch")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "match_expr" => ContinuationSnapshot::MatchExpr {
            arms: match_arms_from_snapshot(required(value, "arms")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "match_stmt" => ContinuationSnapshot::MatchStmt {
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            arms: match_arms_from_snapshot(required(value, "arms")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "handle_handler" => ContinuationSnapshot::HandleHandler {
            handle_expr: HirExprId(required_u32(value, "handle_expr")?),
            body: HirExprId(required_u32(value, "body")?),
            handler: HirExprId(required_u32(value, "handler")?),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "pipeline_input" => ContinuationSnapshot::PipelineInput {
            stages: stages_from_snapshot(required(value, "stages")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "pipeline_target" => ContinuationSnapshot::PipelineTarget {
            input: crate::api::codec::value::snapshot_from_json_with_limits(
                limits,
                required(value, "input")?,
            )
            .map_err(|error| error.to_string())?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "composed_call" => ContinuationSnapshot::ComposedCall {
            remaining: call_target_snapshots_from_json(limits, required(value, "remaining")?)?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "try_expr" => ContinuationSnapshot::TryExpr {
            expr: HirExprId(required_u32(value, "expr")?),
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "memory_clear_delete_all" => ContinuationSnapshot::MemoryClearDeleteAll {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: required_string_array(value, "path")?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "memory_clear_delete_next" => ContinuationSnapshot::MemoryClearDeleteNext {
            region_stable_id: required_str(value, "region_stable_id")?.to_owned(),
            path: required_string_array(value, "path")?,
            remaining_keys: required_values(limits, value, "remaining_keys")?,
            next_index: required_usize(value, "next_index")?,
            span: span_from_snapshot(required(value, "span")?)?,
        },
        "for_loop" => ContinuationSnapshot::ForLoop {
            pat: HirPatId(required_u32(value, "pat")?),
            source: optional_value(limits, value, "source")?,
            next_index: required_usize(value, "next_index")?,
            body: HirBlockId(required_u32(value, "body")?),
            iterations: required_usize(value, "iterations")?,
            loop_scope: required_u32_array(value, "loop_scope")?
                .into_iter()
                .map(SymbolId)
                .collect(),
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "while_loop" => ContinuationSnapshot::WhileLoop {
            cond: HirExprId(required_u32(value, "cond")?),
            body: HirBlockId(required_u32(value, "body")?),
            iteration: required_u32(value, "iteration")?,
            max_iterations: required_u32(value, "max_iterations")?,
            resume_after_body: required_bool(value, "resume_after_body")?,
            span: span_from_snapshot(required(value, "span")?)?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "retry_attempt" => ContinuationSnapshot::RetryAttempt {
            retry: RetryAttemptRecord {
                id: RetryAttemptId(required_u32(value, "retry_id")?),
                ordinal: required_u32(value, "retry_ordinal")?,
            },
            body: HirBlockId(required_u32(value, "body")?),
            attempts: required_usize(value, "attempts")?,
            next_attempt: required_usize(value, "next_attempt")?,
            block: HirBlockId(required_u32(value, "block")?),
            next_stmt_index: required_usize(value, "next_stmt_index")?,
            frame: locals_from_snapshot(limits, required(value, "frame")?)?,
        },
        "agent_prompt_body" => ContinuationSnapshot::AgentPromptBody {
            item: etas_hir::HirItemId(required_u32(value, "item")?),
            span: span_from_snapshot(required(value, "span")?)?,
            model_policy: optional_model_policy(value, "model_policy")?.map(Box::new),
        },
        "restore_model_policy"
        | "handle_boundary"
        | "handler_dispatch"
        | "scoped_model_policy"
        | "call_boundary"
        | "chain" => {
            return Err("continuation decoder expected a leaf, not an owned edge".into());
        }
        "return" => ContinuationSnapshot::Return,
        "resume" => ContinuationSnapshot::Resume,
        "finish" => ContinuationSnapshot::Finish,
        "block_value" => ContinuationSnapshot::BlockValue,
        other => return Err(format!("unknown machine continuation `{other}`")),
    })
}
