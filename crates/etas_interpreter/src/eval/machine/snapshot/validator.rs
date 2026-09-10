use std::collections::BTreeSet;

use etas_frontend::CheckedProject;
use etas_hir::{
    HirArg, HirBlockId, HirEffectArg, HirElseBranch, HirExprId, HirFieldInit, HirItemId,
    HirMapEntry, HirMatchArm, HirMatchArmBody, HirPatId, HirStage, HirTypeId, ResolveResult,
    ResolvedActionRef, ResolvedPath, ScopeId, SymbolId,
};
use etas_types::TypeId;

use crate::orchestration::{
    ActiveHandlerArmRecord, BoundaryOccurrenceId, CallTargetSnapshot, ContinuationSnapshot,
    HandlerScopeId, HandlerSnapshot, InterpreterCheckpoint, LocalPlaceComponentSnapshot,
    LocalPlaceSegmentSnapshot, LocalsSnapshot, MachineFrameSnapshot, MachineSnapshot,
    ModelDecodeSnapshot, ModelLoopFrameSnapshot, SliceExprEvalSnapshot,
    SourceToolReturnFrameSnapshot, ValueSnapshot,
};
use crate::plan::{IntrinsicDispatchTable, SlotLayoutTable};

pub(crate) struct SnapshotValidator<'a> {
    checked: &'a CheckedProject,
    slots: &'a SlotLayoutTable,
    dispatch: &'a IntrinsicDispatchTable,
    limits: &'a etas_host::StorageLimits,
}

impl<'a> SnapshotValidator<'a> {
    pub(crate) fn new(
        checked: &'a CheckedProject,
        slots: &'a SlotLayoutTable,
        dispatch: &'a IntrinsicDispatchTable,
        limits: &'a etas_host::StorageLimits,
    ) -> Self {
        Self {
            checked,
            slots,
            dispatch,
            limits,
        }
    }

    pub(crate) fn validate_checkpoint(
        &self,
        checkpoint: &InterpreterCheckpoint,
    ) -> Result<(), String> {
        for (request, operation) in &checkpoint.storage.operations {
            operation.validate().map_err(|error| error.to_string())?;
            if *request >= checkpoint.trace.next_host_request {
                return Err("checkpoint storage operation refers to a future request".into());
            }
        }
        if checkpoint.storage.operations.len() > self.limits.max_receipts {
            return Err("checkpoint storage operation ledger exceeds limits".into());
        }
        let mut write_ids = BTreeSet::new();
        for write in &checkpoint.storage.writes {
            write
                .evidence
                .operation
                .validate()
                .map_err(|error| error.to_string())?;
            if write.request >= checkpoint.trace.next_host_request
                || !write_ids.insert(write.request)
                || checkpoint.storage.operations.get(&write.request)
                    != Some(&write.evidence.operation)
            {
                return Err(
                    "checkpoint storage evidence does not belong to its boundary occurrence"
                        .to_owned(),
                );
            }
            if let etas_host::CommitStatus::Committed { revision, .. } = &write.evidence.status {
                if revision.starts_with("mv1:") {
                    etas_host::MemoryVersion::parse(revision).map_err(|error| error.to_string())?;
                } else if revision.starts_with("sv1:") {
                    etas_host::session::SessionVersion::parse(revision)
                        .map_err(|error| error.to_string())?;
                } else if revision.starts_with("sg1:") {
                    etas_host::session::SessionGeneration::parse(revision)
                        .map_err(|error| error.to_string())?;
                } else {
                    return Err(
                        "checkpoint storage evidence has an unknown revision kind".to_owned()
                    );
                }
            }
        }
        checkpoint
            .execution_progress
            .original_limits
            .validate()
            .map_err(|message| format!("checkpoint execution limits are invalid: {message}"))?;
        if let Some(max_steps) = checkpoint.execution_progress.original_limits.max_steps
            && checkpoint.execution_progress.consumed_steps > max_steps.get()
        {
            return Err(format!(
                "checkpoint consumed {} execution steps beyond its original limit {max_steps}",
                checkpoint.execution_progress.consumed_steps
            ));
        }
        self.item(checkpoint.entry_item, "checkpoint entry item")?;
        for value in &checkpoint.args {
            self.snapshot_value(&ValueSnapshot::capture(value)?)?;
        }
        let active_scopes = self.handlers(&checkpoint.handlers)?;
        let mut boundary_scopes = BTreeSet::new();
        self.validate_machine_with_handler_scopes(&checkpoint.machine, &mut boundary_scopes)?;
        let expected_boundaries = active_scopes.iter().rev().copied().collect::<Vec<_>>();
        let boundary_unwind_order = self.handler_scope_unwind_order(&checkpoint.machine);
        if boundary_unwind_order != expected_boundaries {
            return Err(format!(
                "checkpoint handler scope topology mismatch: expected unwind order {:?}, found {:?}",
                expected_boundaries, boundary_unwind_order
            ));
        }
        let mut completed_occurrences = BTreeSet::new();
        for completed in &checkpoint.completed_host_boundaries.completed {
            if completed.kind.is_empty() || completed.key.is_empty() {
                return Err("checkpoint completed host boundary has an empty identity".into());
            }
            if !completed_occurrences.insert(completed.occurrence.clone()) {
                return Err(format!(
                    "checkpoint completed host boundary occurrence {:?} is duplicated",
                    completed.occurrence
                ));
            }
            match &completed.occurrence {
                BoundaryOccurrenceId::HostRequest(_) => {}
                BoundaryOccurrenceId::SourceToolCall {
                    call_id,
                    model_request: _,
                } => {
                    if call_id.is_empty() {
                        return Err(
                            "checkpoint completed source tool occurrence has an empty call id"
                                .into(),
                        );
                    }
                }
            }
            if let crate::orchestration::CompletedHostBoundaryResult::Runtime(value) =
                &completed.result
            {
                self.snapshot_value(&ValueSnapshot::capture(value)?)?;
            }
        }
        Ok(())
    }

    pub(crate) fn validate_machine(&self, snapshot: &MachineSnapshot) -> Result<(), String> {
        let mut boundary_scopes = BTreeSet::new();
        self.validate_machine_with_handler_scopes(snapshot, &mut boundary_scopes)
    }

    fn validate_machine_with_handler_scopes(
        &self,
        snapshot: &MachineSnapshot,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        for (index, frame) in snapshot.frames.iter().enumerate() {
            let context = format!("machine frame {index}");
            match frame {
                MachineFrameSnapshot::Block { continuation }
                | MachineFrameSnapshot::Expr { continuation }
                | MachineFrameSnapshot::Call { continuation, .. }
                | MachineFrameSnapshot::Continuation { continuation } => {
                    self.continuation(continuation, &context, boundary_scopes)?;
                }
                MachineFrameSnapshot::Handler { continuation } => {
                    if !matches!(continuation, ContinuationSnapshot::HandleBoundary { .. }) {
                        return Err(format!(
                            "{context} is tagged as handler but does not contain a handler boundary"
                        ));
                    }
                    self.continuation(continuation, &context, boundary_scopes)?;
                }
                MachineFrameSnapshot::Retry { continuation } => {
                    if !matches!(continuation, ContinuationSnapshot::RetryAttempt { .. }) {
                        return Err(format!(
                            "{context} is tagged as retry but does not contain a retry attempt"
                        ));
                    }
                    self.continuation(continuation, &context, boundary_scopes)?;
                }
                MachineFrameSnapshot::ModelLoop(frame) => {
                    self.model_loop(frame, &context, boundary_scopes)?
                }
                MachineFrameSnapshot::SourceToolReturn(frame) => {
                    self.source_tool_return(frame, &context, boundary_scopes)?;
                }
            }
        }
        Ok(())
    }

    fn model_loop(
        &self,
        frame: &ModelLoopFrameSnapshot,
        context: &str,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        match frame.pending.decode {
            ModelDecodeSnapshot::Typed(ty) => {
                self.type_id(ty, &format!("{context} model decode type"))?
            }
            ModelDecodeSnapshot::String | ModelDecodeSnapshot::ModelResponse => {}
        }
        for source_tool in &frame.pending.source_tools {
            self.item(source_tool.item, &format!("{context} source tool binding"))?;
        }
        self.continuation(
            &frame.pending.continuation,
            &format!("{context} pending model continuation"),
            boundary_scopes,
        )?;
        self.continuation(
            &frame.outer_continuation,
            &format!("{context} outer model continuation"),
            boundary_scopes,
        )
    }

    fn source_tool_return(
        &self,
        frame: &SourceToolReturnFrameSnapshot,
        context: &str,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        self.item(frame.binding.item, &format!("{context} source tool item"))?;
        self.model_loop(
            &frame.model_loop,
            &format!("{context} model loop"),
            boundary_scopes,
        )
    }

    fn handlers(&self, snapshot: &HandlerSnapshot) -> Result<Vec<HandlerScopeId>, String> {
        let mut ids = BTreeSet::new();
        let mut ordered_ids = Vec::with_capacity(snapshot.handlers.len());
        let mut previous = None;
        for (index, handler) in snapshot.handlers.iter().enumerate() {
            if !ids.insert(handler.id) {
                return Err(format!(
                    "active handler scope {} is duplicated",
                    handler.id.0
                ));
            }
            if previous.is_some_and(|previous| previous >= handler.id) {
                return Err(format!(
                    "active handler scope order is invalid at scope {}",
                    handler.id.0
                ));
            }
            previous = Some(handler.id);
            ordered_ids.push(handler.id);
            for (arm_index, arm) in handler.handlers.iter().enumerate() {
                self.handler_arm(arm, &format!("active handler {index} arm {arm_index}"))?;
            }
        }
        Ok(ordered_ids)
    }

    fn handler_scope_unwind_order(&self, snapshot: &MachineSnapshot) -> Vec<HandlerScopeId> {
        let mut scopes = Vec::new();
        for frame in snapshot.frames.iter().rev() {
            match frame {
                MachineFrameSnapshot::Block { continuation }
                | MachineFrameSnapshot::Expr { continuation }
                | MachineFrameSnapshot::Call { continuation, .. }
                | MachineFrameSnapshot::Continuation { continuation }
                | MachineFrameSnapshot::Handler { continuation }
                | MachineFrameSnapshot::Retry { continuation } => {
                    Self::continuation_scope_unwind_order(continuation, &mut scopes);
                }
                MachineFrameSnapshot::ModelLoop(frame) => {
                    Self::continuation_scope_unwind_order(&frame.pending.continuation, &mut scopes);
                    Self::continuation_scope_unwind_order(&frame.outer_continuation, &mut scopes);
                }
                MachineFrameSnapshot::SourceToolReturn(frame) => {
                    Self::continuation_scope_unwind_order(
                        &frame.model_loop.pending.continuation,
                        &mut scopes,
                    );
                    Self::continuation_scope_unwind_order(
                        &frame.model_loop.outer_continuation,
                        &mut scopes,
                    );
                }
            }
        }
        scopes
    }

    fn continuation_scope_unwind_order(
        continuation: &ContinuationSnapshot,
        scopes: &mut Vec<HandlerScopeId>,
    ) {
        match continuation {
            ContinuationSnapshot::HandleBoundary {
                scope_id, inner, ..
            } => {
                Self::continuation_scope_unwind_order(inner, scopes);
                scopes.push(*scope_id);
            }
            ContinuationSnapshot::RestoreModelPolicy { inner, .. }
            | ContinuationSnapshot::CallBoundary { outer: inner }
            | ContinuationSnapshot::HandlerDispatch { outer: inner }
            | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => {
                Self::continuation_scope_unwind_order(inner, scopes);
            }
            ContinuationSnapshot::Chain { inner, outer } => {
                Self::continuation_scope_unwind_order(inner, scopes);
                Self::continuation_scope_unwind_order(outer, scopes);
            }
            _ => {}
        }
    }

    fn handler_arm(&self, arm: &ActiveHandlerArmRecord, context: &str) -> Result<(), String> {
        if let Some(symbol) = arm.action_symbol {
            self.symbol(symbol, &format!("{context} action symbol"))?;
        }
        for ty in &arm.type_args {
            self.hir_type(*ty, &format!("{context} type argument"))?;
        }
        for ty in &arm.effect_type_args {
            self.type_id(*ty, &format!("{context} checked type argument"))?;
        }
        for pat in &arm.patterns {
            self.pat(*pat, &format!("{context} pattern"))?;
        }
        self.block(arm.body, &format!("{context} body"))?;
        self.scope(arm.scope, &format!("{context} scope"))
    }

    fn continuation(
        &self,
        continuation: &ContinuationSnapshot,
        context: &str,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        match continuation {
            ContinuationSnapshot::ContinueBlock {
                block,
                next_stmt_index,
                frame,
            } => {
                self.statement_index(*block, *next_stmt_index, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::Bind {
                block,
                next_stmt_index,
                pat,
                frame,
                ..
            } => {
                self.statement_index(*block, *next_stmt_index, context)?;
                self.pat(*pat, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::Assign {
                block,
                next_stmt_index,
                target,
                frame,
                ..
            } => {
                self.statement_index(*block, *next_stmt_index, context)?;
                self.expr(*target, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::AssignTargetIndex {
                block,
                next_stmt_index,
                root_symbol,
                segments,
                components,
                next_component_index,
                new_value,
                frame,
                ..
            } => {
                self.statement_index(*block, *next_stmt_index, context)?;
                self.symbol(*root_symbol, context)?;
                self.local_segments(segments, context)?;
                self.local_components(components, context)?;
                self.index_at_most(*next_component_index, components.len(), context)?;
                self.snapshot_value(new_value)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::FieldReceiver { expr, frame, .. }
            | ContinuationSnapshot::BinaryLeft {
                rhs: expr, frame, ..
            }
            | ContinuationSnapshot::ListConsHead {
                tail: expr, frame, ..
            }
            | ContinuationSnapshot::RangeStart {
                end: expr, frame, ..
            } => {
                self.expr(*expr, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::CalleeEval { args, frame, .. } => {
                self.args(args, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::PipelineInput { stages, frame, .. } => {
                self.stages(stages, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::Unary { .. }
            | ContinuationSnapshot::PromptValueMethodArg { .. }
            | ContinuationSnapshot::Return
            | ContinuationSnapshot::Resume
            | ContinuationSnapshot::Finish
            | ContinuationSnapshot::BlockValue => Ok(()),
            ContinuationSnapshot::BinaryRight { left, .. }
            | ContinuationSnapshot::ListConsTail { head: left, .. }
            | ContinuationSnapshot::RangeEnd { start: left, .. }
            | ContinuationSnapshot::PipelineTarget { input: left, .. } => self.snapshot_value(left),
            ContinuationSnapshot::AggregateElement {
                exprs,
                next_index,
                values,
                frame,
                ..
            } => {
                self.exprs(exprs, context)?;
                self.index_at_most(*next_index, exprs.len(), context)?;
                self.snapshot_values(values)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::RecordField {
                nominal_type,
                fields,
                next_index,
                values,
                frame,
            } => {
                if let Some(ty) = nominal_type {
                    self.type_id(*ty, &format!("{context} nominal record type"))?;
                }
                self.fields(fields, context)?;
                self.index_at_most(*next_index, fields.len(), context)?;
                for (_, value) in values {
                    self.snapshot_value(value)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::MapKey {
                entries,
                index,
                values,
                frame,
            }
            | ContinuationSnapshot::MapValue {
                entries,
                index,
                values,
                frame,
                ..
            } => {
                self.map_entries(entries, context)?;
                self.index_below(*index, entries.len(), context)?;
                for (key, value) in values {
                    self.snapshot_value(key)?;
                    self.snapshot_value(value)?;
                }
                if let ContinuationSnapshot::MapValue { key, .. } = continuation {
                    self.snapshot_value(key)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::IndexBase {
                expr, index, frame, ..
            } => {
                self.expr(*expr, context)?;
                self.expr(*index, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::IndexValue { expr, base, .. } => {
                self.expr(*expr, context)?;
                self.snapshot_value(base)
            }
            ContinuationSnapshot::SliceBase { eval, frame }
            | ContinuationSnapshot::SliceStart { eval, frame, .. } => {
                self.slice_eval(eval, context)?;
                if let ContinuationSnapshot::SliceStart { base, .. } = continuation {
                    self.snapshot_value(base)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::SliceEnd {
                eval, base, start, ..
            } => {
                self.slice_eval(eval, context)?;
                self.snapshot_value(base)?;
                self.snapshot_value(start)
            }
            ContinuationSnapshot::MethodReceiver {
                expr,
                type_args,
                args,
                frame,
                ..
            } => {
                self.expr(*expr, context)?;
                self.hir_types(type_args, context)?;
                self.args(args, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::LocalMethodArgs {
                expr,
                receiver,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            } => {
                self.expr(*expr, context)?;
                self.snapshot_value(receiver)?;
                self.hir_types(type_args, context)?;
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::StaticMethodArgs {
                expr,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            } => {
                self.expr(*expr, context)?;
                self.hir_types(type_args, context)?;
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::SpecMethodReceiver {
                expr,
                receiver_expr,
                spec_symbol,
                spec_args,
                args,
                frame,
                ..
            } => {
                self.expr(*expr, context)?;
                self.expr(*receiver_expr, context)?;
                self.symbol(*spec_symbol, context)?;
                self.hir_types(spec_args, context)?;
                self.args(args, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::PipelineStageTarget {
                stages,
                next_stage_index,
                targets,
                frame,
                ..
            } => {
                self.stages(stages, context)?;
                self.index_at_most(*next_stage_index, stages.len(), context)?;
                for target in targets {
                    self.call_target(target, context)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::CallArgs {
                target,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            } => {
                self.call_target(target, context)?;
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::VariantArgs {
                variant_symbol,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            } => {
                self.symbol(*variant_symbol, context)?;
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::PerformArgs {
                expr,
                action,
                type_args,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            } => {
                self.expr(*expr, context)?;
                self.action(action, context)?;
                self.hir_types(type_args, context)?;
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::MemoryArgs {
                key_type,
                value_type,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            }
            | ContinuationSnapshot::MemorySelectionLimitArgs {
                key_type,
                value_type,
                args,
                next_arg_index,
                evaluated_args,
                frame,
                ..
            } => {
                self.type_id(*key_type, context)?;
                self.type_id(*value_type, context)?;
                if let ContinuationSnapshot::MemoryArgs { result_type, .. } = continuation {
                    self.type_id(*result_type, context)?;
                }
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
                if let ContinuationSnapshot::MemorySelectionLimitArgs {
                    predicate: Some(predicate),
                    ..
                } = continuation
                {
                    self.snapshot_value(predicate)?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::IfExpr {
                then_block,
                else_branch,
                frame,
                ..
            } => {
                self.block(*then_block, context)?;
                self.else_branch(else_branch.as_ref(), context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::IfStmt {
                block,
                next_stmt_index,
                then_block,
                else_branch,
                frame,
                ..
            } => {
                self.statement_index(*block, *next_stmt_index, context)?;
                self.block(*then_block, context)?;
                self.else_branch(else_branch.as_ref(), context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::MatchExpr { arms, frame, .. } => {
                self.match_arms(arms, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::MatchStmt {
                block,
                next_stmt_index,
                arms,
                frame,
                ..
            } => {
                self.statement_index(*block, *next_stmt_index, context)?;
                self.match_arms(arms, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::HandleHandler {
                handle_expr,
                body,
                handler,
                frame,
                ..
            } => {
                self.expr(*handle_expr, context)?;
                self.expr(*body, context)?;
                self.expr(*handler, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::ComposedCall { remaining, .. } => {
                for target in remaining {
                    self.call_target(target, context)?;
                }
                Ok(())
            }
            ContinuationSnapshot::RestoreModelPolicy { inner, .. }
            | ContinuationSnapshot::CallBoundary { outer: inner }
            | ContinuationSnapshot::HandlerDispatch { outer: inner }
            | ContinuationSnapshot::ScopedModelPolicy { inner, .. } => {
                self.continuation(inner, context, boundary_scopes)
            }
            ContinuationSnapshot::ForLoop {
                pat,
                values,
                next_index,
                body,
                loop_scope,
                frame,
                ..
            } => {
                self.pat(*pat, context)?;
                self.block(*body, context)?;
                for symbol in loop_scope {
                    self.symbol(*symbol, context)?;
                }
                if let Some(values) = values {
                    self.index_at_most(*next_index, values.len(), context)?;
                    self.snapshot_values(values)?;
                } else if *next_index != 0 {
                    return Err(format!(
                        "{context} has a for-loop index without materialized values"
                    ));
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::WhileLoop {
                cond, body, frame, ..
            } => {
                self.expr(*cond, context)?;
                self.block(*body, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::RetryAttempt {
                body,
                attempts,
                next_attempt,
                block,
                next_stmt_index,
                frame,
                ..
            } => {
                self.block(*body, context)?;
                self.index_at_most(*next_attempt, *attempts, context)?;
                self.statement_index(*block, *next_stmt_index, context)?;
                self.frame(frame, context)
            }
            ContinuationSnapshot::TryExpr { expr, .. } => self.expr(*expr, context),
            ContinuationSnapshot::MemoryClearDeleteAll { .. } => Ok(()),
            ContinuationSnapshot::MemoryClearDeleteNext {
                remaining_keys,
                next_index,
                ..
            } => {
                self.index_at_most(*next_index, remaining_keys.len(), context)?;
                self.snapshot_values(remaining_keys)
            }
            ContinuationSnapshot::HandleBoundary {
                scope_id,
                inner,
                handlers,
                frame,
                ..
            } => {
                if !boundary_scopes.insert(*scope_id) {
                    return Err(format!(
                        "{context} contains duplicate handle boundary scope {}",
                        scope_id.0
                    ));
                }
                self.continuation(inner, context, boundary_scopes)?;
                for (index, handler) in handlers.iter().enumerate() {
                    self.handler_arm(handler, &format!("{context} handler arm {index}"))?;
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::AgentPromptBody { item, .. } => self.item(*item, context),
            ContinuationSnapshot::Chain { inner, outer } => {
                self.continuation(inner, context, boundary_scopes)?;
                self.continuation(outer, context, boundary_scopes)
            }
        }
    }

    fn call_target(&self, target: &CallTargetSnapshot, context: &str) -> Result<(), String> {
        match target {
            CallTargetSnapshot::FlowItem(item)
            | CallTargetSnapshot::AgentItem(item)
            | CallTargetSnapshot::ToolItem(item) => self.item(*item, context),
            CallTargetSnapshot::SpecImplMethod(symbol) => self.symbol(*symbol, context),
            CallTargetSnapshot::EnumVariant(symbol) => {
                self.symbol(*symbol, context)?;
                if self.dispatch.enum_constructor(*symbol).is_some()
                    || self.checked.symbols.get(*symbol).is_some_and(|symbol| {
                        matches!(symbol.def, etas_hir::SymbolDef::EnumVariant { .. })
                    })
                {
                    Ok(())
                } else {
                    Err(format!(
                        "{context}: call target is not a checked enum constructor"
                    ))
                }
            }
            CallTargetSnapshot::NominalConstructor(ty) => self.type_id(*ty, context),
            CallTargetSnapshot::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            } => {
                self.dispatch
                    .validate_pure_intrinsic(*intrinsic)
                    .map_err(|error| format!("{context}: {error}"))?;
                for parameter_type in parameter_types {
                    self.type_id(*parameter_type, context)?;
                }
                self.type_id(*result_type, context)
            }
            CallTargetSnapshot::Lambda { expr, captured } => {
                self.expr(*expr, context)?;
                self.frame(captured, context)
            }
            CallTargetSnapshot::StdIntrinsic {
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => {
                self.dispatch
                    .resolve_std_callable(crate::intrinsic::dispatch::StdIntrinsicIdentity {
                        intrinsic: *intrinsic,
                        dispatch: *dispatch,
                    })
                    .map_err(|error| format!("{context}: {error}"))?;
                for parameter_type in parameter_types {
                    self.type_id(*parameter_type, context)?;
                }
                self.type_id(*result_type, context)
            }
            CallTargetSnapshot::Specialized {
                target,
                type_bindings,
            } => {
                self.call_target(target, context)?;
                let mut names = std::collections::BTreeSet::new();
                for (name, ty) in type_bindings {
                    if name.is_empty() || !names.insert(name) {
                        return Err(format!(
                            "{context}: specialized call target contains an invalid or duplicate type parameter"
                        ));
                    }
                    self.type_id(*ty, context)?;
                }
                Ok(())
            }
            CallTargetSnapshot::Limited { target, .. } => self.call_target(target, context),
            CallTargetSnapshot::Composed(targets) => {
                for target in targets {
                    self.call_target(target, context)?;
                }
                Ok(())
            }
        }
    }

    fn frame(&self, frame: &LocalsSnapshot, context: &str) -> Result<(), String> {
        let mut type_params = std::collections::BTreeSet::new();
        for (name, ty) in &frame.type_bindings {
            if name.is_empty() || !type_params.insert(name) {
                return Err(format!(
                    "{context} frame contains an invalid or duplicate type parameter binding"
                ));
            }
            self.type_id(*ty, &format!("{context} frame type binding"))?;
        }
        for (symbol, value) in &frame.locals {
            self.symbol(*symbol, &format!("{context} frame local"))?;
            if self.slots.resolve(*symbol).is_none() {
                return Err(format!(
                    "{context} frame local symbol {} has no checked slot layout",
                    symbol.0
                ));
            }
            self.snapshot_value(value)?;
        }
        Ok(())
    }

    fn snapshot_value(&self, value: &ValueSnapshot) -> Result<(), String> {
        match value {
            ValueSnapshot::MemoryWriteIntent(value) => value.validate(self.checked, self.limits),
            ValueSnapshot::Nominal { ty, value } => {
                self.type_id(*ty, "checkpoint nominal value type")?;
                self.snapshot_value(value)
            }
            ValueSnapshot::Trust { value, .. } | ValueSnapshot::OptionSome(value) => {
                self.snapshot_value(value)
            }
            ValueSnapshot::Tuple(values) | ValueSnapshot::Variant { fields: values, .. } => {
                self.snapshot_values(values)
            }
            ValueSnapshot::Message(message) => {
                self.snapshot_value(&message.payload)?;
                Ok(())
            }
            ValueSnapshot::Conversation(conversation) => {
                crate::value::conversation::validate_snapshot(conversation, self.limits)?;
                for message in &conversation.messages {
                    self.snapshot_value(&message.payload)?;
                }
                Ok(())
            }
            ValueSnapshot::Array(values)
            | ValueSnapshot::List(values)
            | ValueSnapshot::Slice(values)
            | ValueSnapshot::Set(values)
            | ValueSnapshot::Deque(values)
            | ValueSnapshot::Queue(values)
            | ValueSnapshot::Stack(values)
            | ValueSnapshot::OrderedSet(values) => self.snapshot_values(values),
            ValueSnapshot::Map(values)
            | ValueSnapshot::PriorityQueue(values)
            | ValueSnapshot::OrderedMap(values) => {
                for (key, value) in values {
                    self.snapshot_value(key)?;
                    self.snapshot_value(value)?;
                }
                Ok(())
            }
            ValueSnapshot::Range { start, end, .. } => {
                self.snapshot_value(start)?;
                self.snapshot_value(end)
            }
            ValueSnapshot::Record(record) => {
                for (_, value) in record {
                    self.snapshot_value(value)?;
                }
                Ok(())
            }
            ValueSnapshot::Callable(target) => {
                self.call_target(target, "checkpoint callable value")
            }
            ValueSnapshot::Handler {
                fact_expr,
                handlers,
            } => {
                self.expr(*fact_expr, "checkpoint handler value fact expression")?;
                for (index, handler) in handlers.iter().enumerate() {
                    self.handler_arm(handler, &format!("checkpoint handler value arm {index}"))?;
                }
                Ok(())
            }
            ValueSnapshot::ResourceHandle { ty, .. } => {
                self.type_id(*ty, "checkpoint resource handle type")
            }
            ValueSnapshot::MemoryStore {
                key_type,
                value_type,
                ..
            }
            | ValueSnapshot::MemorySelection {
                key_type,
                value_type,
                ..
            } => {
                self.type_id(*key_type, "checkpoint memory key type")?;
                self.type_id(*value_type, "checkpoint memory value type")?;
                if let ValueSnapshot::MemorySelection {
                    predicate: Some(predicate),
                    ..
                } = value
                {
                    self.snapshot_value(predicate)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn snapshot_values(&self, values: &[ValueSnapshot]) -> Result<(), String> {
        for value in values {
            self.snapshot_value(value)?;
        }
        Ok(())
    }

    fn local_segments(
        &self,
        values: &[LocalPlaceSegmentSnapshot],
        context: &str,
    ) -> Result<(), String> {
        for value in values {
            if let LocalPlaceSegmentSnapshot::MapKey(value) = value {
                self.snapshot_value(value)?;
            }
        }
        let _ = context;
        Ok(())
    }

    fn local_components(
        &self,
        values: &[LocalPlaceComponentSnapshot],
        context: &str,
    ) -> Result<(), String> {
        for value in values {
            if let LocalPlaceComponentSnapshot::Index { base, index } = value {
                self.expr(*base, context)?;
                self.expr(*index, context)?;
            }
        }
        Ok(())
    }

    fn slice_eval(&self, value: &SliceExprEvalSnapshot, context: &str) -> Result<(), String> {
        self.expr(value.expr, context)?;
        self.expr(value.base, context)?;
        self.expr(value.start, context)?;
        self.expr(value.end, context)
    }

    fn args(&self, args: &[HirArg], context: &str) -> Result<(), String> {
        for arg in args {
            match arg {
                HirArg::Positional(expr) | HirArg::Named { value: expr, .. } => {
                    self.expr(*expr, context)?;
                }
            }
        }
        Ok(())
    }

    fn fields(&self, fields: &[HirFieldInit], context: &str) -> Result<(), String> {
        for field in fields {
            match field {
                HirFieldInit::Shorthand { resolution, .. } => {
                    self.resolve_result(resolution, context)?;
                }
                HirFieldInit::Named { value, .. } => self.expr(*value, context)?,
            }
        }
        Ok(())
    }

    fn map_entries(&self, entries: &[HirMapEntry], context: &str) -> Result<(), String> {
        for entry in entries {
            self.expr(entry.key, context)?;
            self.expr(entry.value, context)?;
        }
        Ok(())
    }

    fn stages(&self, stages: &[HirStage], context: &str) -> Result<(), String> {
        for stage in stages {
            self.expr(stage.expr, context)?;
            self.exprs(&stage.limits, context)?;
        }
        Ok(())
    }

    fn else_branch(&self, branch: Option<&HirElseBranch>, context: &str) -> Result<(), String> {
        match branch {
            Some(HirElseBranch::If(expr)) => self.expr(*expr, context),
            Some(HirElseBranch::Block(block)) => self.block(*block, context),
            None => Ok(()),
        }
    }

    fn match_arms(&self, arms: &[HirMatchArm], context: &str) -> Result<(), String> {
        for arm in arms {
            self.pat(arm.pat, context)?;
            self.scope(arm.scope, context)?;
            match arm.body {
                HirMatchArmBody::Expr(expr) => self.expr(expr, context)?,
                HirMatchArmBody::Block(block) => self.block(block, context)?,
            }
        }
        Ok(())
    }

    fn action(&self, action: &ResolvedActionRef, context: &str) -> Result<(), String> {
        self.path(&action.effect.path, context)?;
        for arg in &action.effect.args {
            match arg {
                HirEffectArg::Type(ty) => self.hir_type(*ty, context)?,
                HirEffectArg::Path(path) => self.path(path, context)?,
                HirEffectArg::Wildcard { .. }
                | HirEffectArg::String { .. }
                | HirEffectArg::Int { .. } => {}
            }
        }
        self.resolve_result(&action.action_symbol, context)
    }

    fn path(&self, path: &ResolvedPath, context: &str) -> Result<(), String> {
        self.resolve_result(&path.resolution, context)
    }

    fn resolve_result(&self, result: &ResolveResult, context: &str) -> Result<(), String> {
        match result {
            ResolveResult::Resolved(symbol) => self.symbol(*symbol, context),
            ResolveResult::PartiallyResolved(partial) => {
                if let Some(symbol) = partial.resolved_prefix {
                    self.symbol(symbol, context)?;
                }
                Err(format!(
                    "{context} contains a partially resolved HIR path in a checked checkpoint"
                ))
            }
            ResolveResult::Unresolved => Err(format!(
                "{context} contains an unresolved HIR path in a checked checkpoint"
            )),
            ResolveResult::Ambiguous(symbols) => {
                for symbol in symbols {
                    self.symbol(*symbol, context)?;
                }
                Err(format!(
                    "{context} contains an ambiguous HIR path in a checked checkpoint"
                ))
            }
        }
    }

    fn exprs(&self, values: &[HirExprId], context: &str) -> Result<(), String> {
        for value in values {
            self.expr(*value, context)?;
        }
        Ok(())
    }

    fn hir_types(&self, values: &[HirTypeId], context: &str) -> Result<(), String> {
        for value in values {
            self.hir_type(*value, context)?;
        }
        Ok(())
    }

    fn statement_index(
        &self,
        block: HirBlockId,
        index: usize,
        context: &str,
    ) -> Result<(), String> {
        let Some(block_value) = self.checked.hir.blocks.get(block) else {
            return Err(format!(
                "{context} references missing HIR block {}",
                block.0
            ));
        };
        self.scope(block_value.scope, context)?;
        self.index_at_most(index, block_value.stmts.len(), context)
    }

    fn index_at_most(&self, index: usize, len: usize, context: &str) -> Result<(), String> {
        if index <= len {
            Ok(())
        } else {
            Err(format!(
                "{context} stores index {index} beyond sequence length {len}"
            ))
        }
    }

    fn index_below(&self, index: usize, len: usize, context: &str) -> Result<(), String> {
        if index < len {
            Ok(())
        } else {
            Err(format!(
                "{context} stores index {index} outside sequence length {len}"
            ))
        }
    }

    fn item(&self, id: HirItemId, context: &str) -> Result<(), String> {
        self.checked
            .hir
            .items
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR item {}", id.0))
    }

    fn block(&self, id: HirBlockId, context: &str) -> Result<(), String> {
        self.checked
            .hir
            .blocks
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR block {}", id.0))
    }

    fn expr(&self, id: HirExprId, context: &str) -> Result<(), String> {
        self.checked
            .hir
            .exprs
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR expression {}", id.0))
    }

    fn pat(&self, id: HirPatId, context: &str) -> Result<(), String> {
        self.checked
            .hir
            .pats
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR pattern {}", id.0))
    }

    fn hir_type(&self, id: HirTypeId, context: &str) -> Result<(), String> {
        self.checked
            .hir
            .types
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR type {}", id.0))
    }

    fn symbol(&self, id: SymbolId, context: &str) -> Result<(), String> {
        self.checked
            .symbols
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR symbol {}", id.0))
    }

    fn scope(&self, id: ScopeId, context: &str) -> Result<(), String> {
        self.checked
            .scopes
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing HIR scope {}", id.0))
    }

    fn type_id(&self, id: TypeId, context: &str) -> Result<(), String> {
        self.checked
            .type_store
            .get(id)
            .map(|_| ())
            .ok_or_else(|| format!("{context} references missing checked type {}", id.0))
    }
}

#[cfg(test)]
mod tests {
    use etas_core::{SourceId, Span, TextRange, TextSize};
    use etas_hir::{HirBlockId, HirExprId, HirItemId, HirPatId, HirTypeId, ScopeId, SymbolId};
    use etas_types::TypeId;

    use crate::orchestration::{
        ActiveHandlerArmRecord, CallTargetSnapshot, ContinuationSnapshot, LocalsSnapshot,
        MachineFrameSnapshot, MachineSnapshot,
    };
    use crate::plan::SlotLayoutTable;

    use super::SnapshotValidator;

    fn checked_project() -> etas_frontend::CheckedProject {
        crate::testing::project::checked_project(
            r#"
module app.main;

flow helper(value: i32) -> i32 {
  return value;
}

flow main() -> unit {
  let value: i32 = 1;
  helper(value);
  return;
}
"#,
        )
    }

    fn span() -> Span {
        Span::new(SourceId(0), TextRange::new(TextSize(0), TextSize(1)))
    }

    fn empty_locals() -> LocalsSnapshot {
        LocalsSnapshot {
            locals: Vec::new(),
            type_bindings: Vec::new(),
        }
    }

    fn validate_continuation(
        checked: &etas_frontend::CheckedProject,
        continuation: ContinuationSnapshot,
    ) -> Result<(), String> {
        let slots = SlotLayoutTable::for_project(checked);
        let dispatch = crate::plan::IntrinsicDispatchTable::for_project(checked)
            .map_err(|errors| errors.join("; "))?;
        SnapshotValidator::new(
            checked,
            &slots,
            &dispatch,
            &etas_host::StorageLimits::default(),
        )
        .validate_machine(&MachineSnapshot {
            frames: vec![MachineFrameSnapshot::Continuation { continuation }],
        })
    }

    #[test]
    fn rejects_every_hir_id_class_and_statement_index_before_restore() {
        let checked = checked_project();
        let block = checked.hir.blocks.iter().next().expect("block").0;
        let expr = checked.hir.exprs.iter().next().expect("expr").0;
        let pat = checked.hir.pats.iter().next().expect("pattern").0;

        let cases = [
            (
                ContinuationSnapshot::ContinueBlock {
                    block: HirBlockId(u32::MAX),
                    next_stmt_index: 0,
                    frame: empty_locals(),
                },
                "missing HIR block",
            ),
            (
                ContinuationSnapshot::ContinueBlock {
                    block,
                    next_stmt_index: usize::MAX,
                    frame: empty_locals(),
                },
                "beyond sequence length",
            ),
            (
                ContinuationSnapshot::FieldReceiver {
                    expr: HirExprId(u32::MAX),
                    field: "value".to_owned(),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing HIR expression",
            ),
            (
                ContinuationSnapshot::Bind {
                    block,
                    next_stmt_index: 0,
                    pat: HirPatId(u32::MAX),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing HIR pattern",
            ),
            (
                ContinuationSnapshot::MethodReceiver {
                    expr,
                    method: "method".to_owned(),
                    type_args: vec![HirTypeId(u32::MAX)],
                    args: Vec::new(),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing HIR type",
            ),
            (
                ContinuationSnapshot::MemoryArgs {
                    result_type: TypeId(u32::MAX),
                    region_stable_id: "region".to_owned(),
                    path: Vec::new(),
                    key_type: TypeId(u32::MAX),
                    value_type: TypeId(u32::MAX),
                    method: "get".to_owned(),
                    args: Vec::new(),
                    next_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing checked type",
            ),
            (
                ContinuationSnapshot::SpecMethodReceiver {
                    expr,
                    receiver_expr: expr,
                    spec_symbol: SymbolId(u32::MAX),
                    spec_args: Vec::new(),
                    method: "method".to_owned(),
                    args: Vec::new(),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing HIR symbol",
            ),
            (
                ContinuationSnapshot::CallArgs {
                    target: CallTargetSnapshot::ToolItem(HirItemId(u32::MAX)),
                    args: Vec::new(),
                    next_arg_index: 0,
                    evaluated_args: Vec::new(),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing HIR item",
            ),
        ];

        for (continuation, expected) in cases {
            let error = validate_continuation(&checked, continuation)
                .expect_err("invalid snapshot ID must fail closed");
            assert!(
                error.contains(expected),
                "expected `{expected}` in `{error}`"
            );
        }

        // Keep the valid IDs used above live so this test cannot pass because the fixture is empty.
        assert!(checked.hir.exprs.get(expr).is_some());
        assert!(checked.hir.pats.get(pat).is_some());
    }

    #[test]
    fn rejects_invalid_scope_in_nested_handler_snapshot() {
        let checked = checked_project();
        let block = checked.hir.blocks.iter().next().expect("block").0;
        let slots = SlotLayoutTable::for_project(&checked);
        let dispatch = crate::plan::IntrinsicDispatchTable::for_project(&checked)
            .expect("test intrinsic dispatch");
        let limits = etas_host::StorageLimits::default();
        let validator = SnapshotValidator::new(&checked, &slots, &dispatch, &limits);
        let error = validator
            .handler_arm(
                &ActiveHandlerArmRecord {
                    effect_segments: vec!["Gate".to_owned()],
                    action: "request".to_owned(),
                    action_symbol: None,
                    type_args: Vec::new(),
                    effect_type_args: Vec::new(),
                    patterns: Vec::new(),
                    body: block,
                    scope: ScopeId(u32::MAX),
                    span: span(),
                },
                "test handler",
            )
            .expect_err("invalid handler scope must fail closed");
        assert!(error.contains("missing HIR scope"));
    }
}
