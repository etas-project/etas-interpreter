use super::support::*;
use super::*;
use crate::api::codec::snapshot::{Node, Pending, write_object};
use crate::orchestration::ContinuationSnapshot as Continuation;

macro_rules! object {
    ($slot:ident, $pending:ident; $($key:literal: $value:expr),* $(,)?) => {
        write_object([$(($key, ($value).into())),*], $slot, $pending)
    };
}
pub(in crate::api::codec) fn write_continuation<'a>(
    continuation: &'a Continuation,
    slot: &'a mut Value,
    pending: &mut Pending<'a>,
) {
    match continuation {
        Continuation::ContinueBlock {
            block,
            next_stmt_index,
            frame,
        } => object! { slot, pending;
            "kind": json!("continue_block"),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "frame": Node::Frame(frame),
        },
        Continuation::Bind {
            block,
            next_stmt_index,
            pat,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("bind"),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "pat": json!(pat.0),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::Assign {
            block,
            next_stmt_index,
            target,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("assign"),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "target": json!(target.0),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
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
        } => object! { slot, pending;
            "kind": json!("assign_target_index"),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "root_symbol": json!(root_symbol.0),
            "segments": Node::LocalSegments(segments),
            "components": json!(components.iter().map(local_place_component_snapshot).collect::<Vec<_>>()),
            "next_component_index": json!(next_component_index),
            "new_value": Node::Value(new_value),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::FieldReceiver {
            expr,
            field,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("field_receiver"),
            "expr": json!(expr.0),
            "field": json!(field),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::Unary { op, span } => {
            object! { slot, pending;
            "kind": json!("unary"),
                "op": json!(unary_op_name(*op)),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::BinaryLeft {
            op,
            rhs,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("binary_left"),
            "op": json!(binary_op_name(*op)),
            "rhs": json!(rhs.0),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::BinaryRight { op, left, span } => {
            object! { slot, pending;
            "kind": json!("binary_right"),
                "op": json!(binary_op_name(*op)),
                "left": Node::Value(left),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::AggregateElement {
            expr,
            next_index,
            values,
            frame,
        } => object! { slot, pending;
            "kind": json!("aggregate_element"),
            "expr": json!(expr.0),
            "next_index": json!(next_index),
            "values": Node::Values(values),
            "frame": Node::Frame(frame),
        },
        Continuation::ListConsHead { tail, span, frame } => {
            object! { slot, pending;
            "kind": json!("list_cons_head"),
                "tail": json!(tail.0),
                "span": json!(span_snapshot(*span)),
                "frame": Node::Frame(frame),
            }
        }
        Continuation::ListConsTail { head, span } => {
            object! { slot, pending;
            "kind": json!("list_cons_tail"),
                "head": Node::Value(head),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::RangeStart { end, bounds, frame } => {
            object! { slot, pending;
            "kind": json!("range_start"),
                "end": json!(end.0),
                "bounds": json!(range_bounds_name(*bounds)),
                "frame": Node::Frame(frame),
            }
        }
        Continuation::RangeEnd { start, bounds } => {
            object! { slot, pending;
            "kind": json!("range_end"),
                "start": Node::Value(start),
                "bounds": json!(range_bounds_name(*bounds)),
            }
        }
        Continuation::RecordField {
            expr,
            nominal_type,
            variant_symbol,
            next_index,
            values,
            frame,
        } => object! { slot, pending;
            "kind": json!("record_field"),
            "expr": json!(expr.0),
            "nominal_type": json!(nominal_type.map(|ty| ty.0)),
            "variant_symbol": json!(variant_symbol.map(|symbol| symbol.0)),
            "next_index": json!(next_index),
            "values": Node::NamedValues(values),
            "frame": Node::Frame(frame),
        },
        Continuation::MapKey {
            expr,
            index,
            values,
            frame,
        } => object! { slot, pending;
            "kind": json!("map_key"),
            "expr": json!(expr.0),
            "index": json!(index),
            "values": Node::Pairs(values),
            "frame": Node::Frame(frame),
        },
        Continuation::MapValue {
            expr,
            index,
            key,
            values,
            frame,
        } => object! { slot, pending;
            "kind": json!("map_value"),
            "expr": json!(expr.0),
            "index": json!(index),
            "key": Node::Value(key),
            "values": Node::Pairs(values),
            "frame": Node::Frame(frame),
        },
        Continuation::IndexBase {
            expr,
            index,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("index_base"),
            "expr": json!(expr.0),
            "index": json!(index.0),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::IndexValue { expr, base, span } => {
            object! { slot, pending;
            "kind": json!("index_value"),
                "expr": json!(expr.0),
                "base": Node::Value(base),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::SliceBase { eval, frame } => {
            object! { slot, pending;
            "kind": json!("slice_base"),
                "eval": json!(slice_eval_snapshot(eval)),
                "frame": Node::Frame(frame),
            }
        }
        Continuation::SliceStart { eval, base, frame } => {
            object! { slot, pending;
            "kind": json!("slice_start"),
                "eval": json!(slice_eval_snapshot(eval)),
                "base": Node::Value(base),
                "frame": Node::Frame(frame),
            }
        }
        Continuation::SliceEnd { eval, base, start } => {
            object! { slot, pending;
            "kind": json!("slice_end"),
                "eval": json!(slice_eval_snapshot(eval)),
                "base": Node::Value(base),
                "start": Node::Value(start),
            }
        }
        Continuation::MethodReceiver {
            expr,
            method,
            type_args,
            args,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("method_receiver"),
            "expr": json!(expr.0),
            "method": json!(method),
            "type_args": json!(type_args.iter().map(|ty| ty.0).collect::<Vec<_>>()),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::PromptValueMethodArg {
            messages,
            method,
            role,
            allow_plain_system_content,
            span,
        } => object! { slot, pending;
            "kind": json!("prompt_value_method_arg"),
            "messages": json!(messages.iter().map(prompt_message_snapshot).collect::<Vec<_>>()),
            "method": json!(method),
            "role": json!(prompt_role_name(*role)),
            "allow_plain_system_content": json!(allow_plain_system_content),
            "span": json!(span_snapshot(*span)),
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
        } => object! { slot, pending;
            "kind": json!("local_method_args"),
            "expr": json!(expr.0),
            "receiver": Node::Value(receiver),
            "method": json!(method),
            "type_args": json!(type_args.iter().map(|ty| ty.0).collect::<Vec<_>>()),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
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
        } => object! { slot, pending;
            "kind": json!("static_method_args"),
            "expr": json!(expr.0),
            "static_kind": json!(static_method_kind_snapshot(kind)),
            "method": json!(method),
            "type_args": json!(type_args.iter().map(|ty| ty.0).collect::<Vec<_>>()),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
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
        } => object! { slot, pending;
            "kind": json!("spec_method_receiver"),
            "expr": json!(expr.0),
            "receiver_expr": json!(receiver_expr.0),
            "spec_symbol": json!(spec_symbol.0),
            "spec_args": json!(spec_args.iter().map(|ty| ty.0).collect::<Vec<_>>()),
            "method": json!(method),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::CalleeEval { args, span, frame } => {
            object! { slot, pending;
            "kind": json!("callee_eval"),
                "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
                "span": json!(span_snapshot(*span)),
                "frame": Node::Frame(frame),
            }
        }
        Continuation::PipelineStageTarget {
            stages,
            next_stage_index,
            targets,
            current_limits,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("pipeline_stage_target"),
            "stages": json!(stages.iter().map(stage_snapshot).collect::<Vec<_>>()),
            "next_stage_index": json!(next_stage_index),
            "targets": Node::CallTargets(targets),
            "current_limits": json!(current_limits.iter().map(runtime_limit_snapshot).collect::<Vec<_>>()),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::CallArgs {
            target,
            args,
            next_arg_index,
            evaluated_args,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("call_args"),
            "target": Node::CallTarget(target),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::VariantArgs {
            variant_symbol,
            args,
            next_arg_index,
            evaluated_args,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("variant_args"),
            "variant_symbol": json!(variant_symbol.0),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::PerformArgs {
            expr,
            type_args,
            next_arg_index,
            evaluated_args,
            frame,
            ..
        } => object! { slot, pending;
            "kind": json!("perform_args"),
            "expr": json!(expr.0),
            "type_args": json!(type_args.iter().map(|ty| ty.0).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "frame": Node::Frame(frame),
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
        } => object! { slot, pending;
            "kind": json!("memory_args"),
            "result_type": json!(result_type.0),
            "region_stable_id": json!(region_stable_id),
            "path": json!(path),
            "key_type": json!(key_type.0),
            "value_type": json!(value_type.0),
            "method": json!(method),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
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
        } => object! { slot, pending;
            "kind": json!("memory_selection_limit_args"),
            "region_stable_id": json!(region_stable_id),
            "path": json!(path),
            "key_type": json!(key_type.0),
            "value_type": json!(value_type.0),
            "selection_kind": json!(crate::value::codec::memory_selection_kind_json(kind)),
            "predicate": predicate.as_ref().map(Node::Value),
            "limit": json!(limit),
            "args": json!(args.iter().map(arg_snapshot).collect::<Vec<_>>()),
            "next_arg_index": json!(next_arg_index),
            "evaluated_args": Node::Values(evaluated_args),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::IfExpr {
            then_block,
            else_branch,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("if_expr"),
            "then_block": json!(then_block.0),
            "else_branch": json!(else_branch.as_ref().map(else_branch_snapshot)),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::IfStmt {
            block,
            next_stmt_index,
            then_block,
            else_branch,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("if_stmt"),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "then_block": json!(then_block.0),
            "else_branch": json!(else_branch.as_ref().map(else_branch_snapshot)),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::MatchExpr { arms, span, frame } => {
            object! { slot, pending;
            "kind": json!("match_expr"),
                "arms": json!(arms.iter().map(match_arm_snapshot).collect::<Vec<_>>()),
                "span": json!(span_snapshot(*span)),
                "frame": Node::Frame(frame),
            }
        }
        Continuation::MatchStmt {
            block,
            next_stmt_index,
            arms,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("match_stmt"),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "arms": json!(arms.iter().map(match_arm_snapshot).collect::<Vec<_>>()),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::HandleHandler {
            handle_expr,
            body,
            handler,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("handle_handler"),
            "handle_expr": json!(handle_expr.0),
            "body": json!(body.0),
            "handler": json!(handler.0),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::PipelineInput {
            stages,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("pipeline_input"),
            "stages": json!(stages.iter().map(stage_snapshot).collect::<Vec<_>>()),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::PipelineTarget { input, span } => {
            object! { slot, pending;
            "kind": json!("pipeline_target"),
                "input": Node::Value(input),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::ComposedCall { remaining, span } => {
            object! { slot, pending;
            "kind": json!("composed_call"),
                "remaining": Node::CallTargets(remaining),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::RestoreModelPolicy { previous, inner } => {
            object! { slot, pending;
            "kind": json!("restore_model_policy"),
                "previous": json!(model_policy_snapshot(previous)),
                "inner": Node::Continuation(inner),
            }
        }
        Continuation::TryExpr { expr, span } => {
            object! { slot, pending;
            "kind": json!("try_expr"),
                "expr": json!(expr.0),
                "span": json!(span_snapshot(*span)),
            }
        }
        Continuation::MemoryClearDeleteAll {
            region_stable_id,
            path,
            span,
        } => object! { slot, pending;
            "kind": json!("memory_clear_delete_all"),
            "region_stable_id": json!(region_stable_id),
            "path": json!(path),
            "span": json!(span_snapshot(*span)),
        },
        Continuation::MemoryClearDeleteNext {
            region_stable_id,
            path,
            remaining_keys,
            next_index,
            span,
        } => object! { slot, pending;
            "kind": json!("memory_clear_delete_next"),
            "region_stable_id": json!(region_stable_id),
            "path": json!(path),
            "remaining_keys": Node::Values(remaining_keys),
            "next_index": json!(next_index),
            "span": json!(span_snapshot(*span)),
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
            let mut scope = loop_scope.iter().map(|symbol| symbol.0).collect::<Vec<_>>();
            scope.sort_unstable();
            object! { slot, pending;
            "kind": json!("for_loop"),
                "pat": json!(pat.0),
                "source": source.as_ref().map(Node::Value),
                "next_index": json!(next_index),
                "body": json!(body.0),
                "iterations": json!(iterations),
                "loop_scope": json!(scope),
                "span": json!(span_snapshot(*span)),
                "frame": Node::Frame(frame),
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
        } => object! { slot, pending;
            "kind": json!("while_loop"),
            "cond": json!(cond.0),
            "body": json!(body.0),
            "iteration": json!(iteration),
            "max_iterations": json!(max_iterations),
            "resume_after_body": json!(resume_after_body),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::RetryAttempt {
            retry,
            body,
            attempts,
            next_attempt,
            block,
            next_stmt_index,
            frame,
        } => object! { slot, pending;
            "kind": json!("retry_attempt"),
            "retry_id": json!(retry.id.0),
            "retry_ordinal": json!(retry.ordinal),
            "body": json!(body.0),
            "attempts": json!(attempts),
            "next_attempt": json!(next_attempt),
            "block": json!(block.0),
            "next_stmt_index": json!(next_stmt_index),
            "frame": Node::Frame(frame),
        },
        Continuation::HandleBoundary {
            scope_id,
            inner,
            handlers,
            span,
            frame,
        } => object! { slot, pending;
            "kind": json!("handle_boundary"),
            "scope_id": json!(scope_id.0),
            "inner": Node::Continuation(inner),
            "handlers": json!(handlers.iter().map(handler_arm_snapshot).collect::<Vec<_>>()),
            "span": json!(span_snapshot(*span)),
            "frame": Node::Frame(frame),
        },
        Continuation::HandlerDispatch { outer } => {
            object! { slot, pending;
            "kind": json!("handler_dispatch"),
                "outer": Node::Continuation(outer),
            }
        }
        Continuation::AgentPromptBody {
            item,
            span,
            model_policy,
        } => object! { slot, pending;
            "kind": json!("agent_prompt_body"),
            "item": json!(item.0),
            "span": json!(span_snapshot(*span)),
            "model_policy": json!(model_policy.as_ref().map(|policy| model_policy_snapshot(policy))),
        },
        Continuation::ScopedModelPolicy { policy, inner } => {
            object! { slot, pending;
            "kind": json!("scoped_model_policy"),
                "policy": json!(model_policy_snapshot(policy)),
                "inner": Node::Continuation(inner),
            }
        }
        Continuation::CallBoundary { outer } => {
            object! { slot, pending;
            "kind": json!("call_boundary"),
                "outer": Node::Continuation(outer),
            }
        }
        Continuation::Chain { inner, outer } => {
            object! { slot, pending;
            "kind": json!("chain"),
                "inner": Node::Continuation(inner),
                "outer": Node::Continuation(outer),
            }
        }
        Continuation::Return => object! { slot, pending;
            "kind": json!("return"),
        },
        Continuation::Resume => object! { slot, pending;
            "kind": json!("resume"),
        },
        Continuation::Finish => object! { slot, pending;
            "kind": json!("finish"),
        },
        Continuation::BlockValue => {
            object! { slot, pending;
            "kind": json!("block_value"),
            }
        }
    }
}
