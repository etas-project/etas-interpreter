use std::collections::BTreeSet;

mod access;
mod bounded_filesystem_reducer;
mod call_site;
mod call_target;
mod callable_types;
mod comparison;
mod construction;
mod frame;
mod input;
mod program;
mod reference_presence;
mod reference_types;
mod residual_coverage;
mod revision;
mod scoped_callable;
mod stream_reducer;
mod suspension;
mod task;
mod traversal;
mod value_contract;

#[cfg(test)]
mod call_target_tests;
#[cfg(test)]
mod shared_call_target_tests;
#[cfg(test)]
mod traversal_tests;

use etas_frontend::CheckedProject;
use etas_hir::{
    HirArg, HirBlockId, HirEffectArg, HirElseBranch, HirExprId, HirFieldInit, HirItemId,
    HirMatchArm, HirMatchArmBody, HirPatId, HirTypeId, ResolveResult, ResolvedActionRef,
    ResolvedPath, ScopeId, SymbolId,
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
    inherited_handlers: usize,
    tasks: Option<&'a crate::driver::scheduler::TaskSchedulerMetadataSnapshot>,
    task_identity: Option<crate::driver::scheduler::TaskIdentity>,
    execution_revision: Option<&'a crate::api::code::PreparedCodeRevision>,
    revisions: Option<&'a crate::api::code::PreparedRevisionSet>,
    frame_definitions:
        std::rc::Rc<std::cell::RefCell<std::collections::HashMap<u64, frame::FrameValidation>>>,
    frame_owners: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<u64, usize>>>,
    suspension_references: std::rc::Rc<std::cell::RefCell<Vec<(u32, CheckedProject)>>>,
    residual_references: std::rc::Rc<std::cell::RefCell<ResidualReferences>>,
    validated_call_targets: std::cell::RefCell<call_target::ValidatedCallTargets>,
    reference_presence: std::cell::RefCell<reference_presence::ReferencePresence>,
    callable_specs: std::cell::OnceCell<etas_types::SpecFacts>,
    typed_callables: std::cell::RefCell<callable_types::TypedCallables>,
    checked: &'a CheckedProject,
    slots: &'a SlotLayoutTable,
    dispatch: &'a IntrinsicDispatchTable,
    closures: &'a crate::plan::ClosureLayoutTable,
    limits: &'a etas_host::StorageLimits,
}

impl<'a> SnapshotValidator<'a> {
    pub(crate) fn new(
        checked: &'a CheckedProject,
        slots: &'a SlotLayoutTable,
        dispatch: &'a IntrinsicDispatchTable,
        closures: &'a crate::plan::ClosureLayoutTable,
        limits: &'a etas_host::StorageLimits,
    ) -> Self {
        Self {
            inherited_handlers: 0,
            tasks: None,
            task_identity: None,
            execution_revision: None,
            revisions: None,
            frame_definitions: Default::default(),
            frame_owners: Default::default(),
            suspension_references: Default::default(),
            residual_references: Default::default(),
            validated_call_targets: Default::default(),
            reference_presence: Default::default(),
            callable_specs: Default::default(),
            typed_callables: Default::default(),
            checked,
            slots,
            dispatch,
            closures,
            limits,
        }
    }

    pub(crate) fn with_revisions(
        mut self,
        revisions: Option<&'a crate::api::code::PreparedRevisionSet>,
    ) -> Self {
        self.revisions = revisions;
        self
    }

    pub(crate) fn with_tasks(
        mut self,
        tasks: Option<&'a crate::driver::scheduler::TaskSchedulerMetadataSnapshot>,
        task: Option<crate::driver::scheduler::TaskIdentity>,
    ) -> Self {
        self.tasks = tasks;
        self.task_identity = task;
        self
    }

    pub(crate) fn with_inherited_handlers(mut self, depth: usize) -> Self {
        self.inherited_handlers = depth;
        self
    }

    pub(crate) fn with_execution_revision(
        mut self,
        revision: Option<&'a crate::api::code::PreparedCodeRevision>,
    ) -> Self {
        self.execution_revision = revision;
        self
    }

    pub(crate) fn validate_arguments(
        &self,
        item: HirItemId,
        values: &[ValueSnapshot],
    ) -> Result<(), String> {
        for value in values {
            self.snapshot_value_in(value, call_target::ReferenceScope::CheckedSlot)?;
        }
        self.validate_argument_contract(item, values)?;
        Ok(())
    }

    pub(crate) fn validate_checkpoint(
        &self,
        checkpoint: &InterpreterCheckpoint,
    ) -> Result<(), String> {
        checkpoint.model_policy.validate()?;
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
        self.validate_root_revision(checkpoint)?;
        self.validate_arguments(checkpoint.entry_item, &checkpoint.args)?;
        let mut boundary_scopes = BTreeSet::new();
        self.validate_machine_with_handler_scopes(&checkpoint.machine, &mut boundary_scopes)?;
        self.validate_root_residual_coverage(checkpoint)?;
        if checkpoint.machine.total_call_frames()?
            > checkpoint
                .execution_progress
                .original_limits
                .max_call_depth
                .get()
        {
            return Err(
                "checkpoint active and suspended calls exceed its original call-depth limit".into(),
            );
        }
        let active_scopes = self.validate_program_handlers(checkpoint)?;
        let expected_boundaries = active_scopes.iter().rev().copied().collect::<Vec<_>>();
        let mut boundary_unwind_order = Vec::new();
        let mut retries = Vec::new();
        let pending = checkpoint
            .tasks
            .as_deref()
            .map(|tasks| tasks.root_continuation(&checkpoint.machine))
            .transpose()?
            .flatten()
            .or_else(|| {
                checkpoint
                    .machine
                    .input
                    .as_deref()
                    .and_then(|input| input.continuation_root())
            });
        if let Some(pending) = pending {
            Self::continuation_unwind_order(pending, &mut boundary_unwind_order, &mut retries);
        }
        Self::frame_unwind_order(
            &checkpoint.machine.frames,
            &mut boundary_unwind_order,
            &mut retries,
        );
        if boundary_unwind_order != expected_boundaries {
            return Err(format!(
                "checkpoint handler scope topology mismatch: expected unwind order {:?}, found {:?}",
                expected_boundaries, boundary_unwind_order
            ));
        }
        if retries
            != checkpoint
                .retry_state
                .attempts
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
        {
            return Err("checkpoint retry scope topology mismatch".into());
        }
        let mut retry_ids = BTreeSet::new();
        for retry in checkpoint.retry_state.attempts.iter().chain(
            checkpoint
                .machine
                .suspended_programs
                .iter()
                .flat_map(|node| &node.retries.attempts),
        ) {
            if retry.id.0 == u32::MAX || !retry_ids.insert(retry.id.0) {
                return Err("checkpoint retry scope identity is exhausted or duplicated".into());
            }
        }
        let mut completed_occurrences = BTreeSet::new();
        for completed in &checkpoint.completed_host_boundaries.completed {
            if completed.kind.is_empty() {
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

    pub(crate) fn validate_callable_target(
        &self,
        target: &CallTargetSnapshot,
    ) -> Result<(), String> {
        self.call_target(target, "checkpoint native callable")
    }

    fn validate_machine_with_handler_scopes(
        &self,
        snapshot: &MachineSnapshot,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        self.suspension_references.borrow_mut().clear();
        *self.residual_references.borrow_mut() = ResidualReferences::default();
        self.validate_program_frames(snapshot, boundary_scopes)?;
        self.validate_machine_input(snapshot)?;
        self.validate_auxiliary(snapshot)?;
        self.validate_suspensions(snapshot, boundary_scopes)?;
        self.validate_residual_references(snapshot)
    }

    /// Every residual scope and attempt reference must name an entry of the
    /// checkpoint's bounded residual tables, and every table entry must be
    /// referenced: a live scope state cannot float without its owner.
    fn validate_residual_references(&self, snapshot: &MachineSnapshot) -> Result<(), String> {
        let residual = &snapshot.residual;
        residual.validate_counters()?;
        for record in &residual.exits.records {
            let item = etas_hir::HirItemId(record.item);
            self.item(item, "checkpoint residual exit history")?;
            let revision = self
                .checked
                .residual_plans()
                .binding()
                .and_then(|binding| binding.revision(item))
                .ok_or("checkpoint residual exit history has no checked revision")?;
            if revision.key() != record.revision {
                return Err("checkpoint residual exit history names an unbound revision".into());
            }
        }
        if residual.scopes.len() > crate::orchestration::MAX_CHECKPOINT_RESIDUAL_SCOPES
            || residual.attempts.len() > crate::orchestration::MAX_CHECKPOINT_RESIDUAL_ATTEMPTS
        {
            return Err("checkpoint residual state exceeds resource limits".into());
        }
        let references = self.residual_references.borrow();
        let mut scopes = vec![false; residual.scopes.len()];
        let mut attempts = vec![false; residual.attempts.len()];
        for scope in &references.scopes {
            *scopes
                .get_mut(*scope as usize)
                .ok_or("checkpoint references an unknown residual scope")? = true;
        }
        for attempt in &references.attempts {
            *attempts
                .get_mut(*attempt as usize)
                .ok_or("checkpoint references an unknown residual attempt")? = true;
        }
        for attempt in &residual.attempts {
            if attempt.scopes.is_empty()
                || attempt.scopes.len() > crate::orchestration::MAX_CHECKPOINT_RESIDUAL_SCOPES
            {
                return Err("checkpoint residual attempt has no bounded observing scopes".into());
            }
            for observed in &attempt.scopes {
                if observed.scope as usize >= residual.scopes.len() {
                    return Err("checkpoint residual attempt names an unknown scope".into());
                }
            }
        }
        if scopes.iter().any(|used| !used) || attempts.iter().any(|used| !used) {
            return Err("checkpoint residual state has an unowned entry".into());
        }
        for scope in &residual.scopes {
            if scope.outcomes.len() > crate::orchestration::MAX_CHECKPOINT_RESIDUAL_ENTRIES
                || scope.pending.len() > crate::orchestration::MAX_CHECKPOINT_RESIDUAL_ENTRIES
                || scope.states.len() > crate::orchestration::MAX_CHECKPOINT_RESIDUAL_ENTRIES
            {
                return Err("checkpoint residual scope exceeds resource limits".into());
            }
            self.item(etas_hir::HirItemId(scope.item), "checkpoint residual scope")?;
        }
        Ok(())
    }

    fn validate_local_frames(
        &self,
        frames: &[MachineFrameSnapshot],
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        for (index, frame) in frames.iter().enumerate() {
            let context = format!("machine frame {index}");
            match frame {
                MachineFrameSnapshot::ProgramReturn(frame) => {
                    self.invocation_site(&frame.call_site, frame.span, &context)?;
                    self.continuation(&frame.outer, &context, boundary_scopes)?;
                }
                MachineFrameSnapshot::Block { continuation }
                | MachineFrameSnapshot::Expr { continuation }
                | MachineFrameSnapshot::Continuation { continuation } => {
                    self.continuation(continuation, &context, boundary_scopes)?;
                }
                MachineFrameSnapshot::Call {
                    caller,
                    continuation,
                    span,
                } => {
                    if let Some(caller) = caller {
                        self.call_origin(caller, *span, &context)?;
                    }
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
                MachineFrameSnapshot::ResidualScope { scope } => {
                    self.residual_references.borrow_mut().scopes.push(*scope);
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
        if frame.boundary_request_id.0 > frame.pending.request.id.0 {
            return Err(format!(
                "{context} model loop request precedes its original boundary occurrence"
            ));
        }
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
            if handler.id.0 == u32::MAX {
                return Err("checkpoint handler scope identity space is exhausted".into());
            }
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

    #[cfg(test)]
    fn handler_scope_unwind_order(&self, snapshot: &MachineSnapshot) -> Vec<HandlerScopeId> {
        let mut scopes = Vec::new();
        Self::frame_unwind_order(&snapshot.frames, &mut scopes, &mut Vec::new());
        scopes
    }

    fn frame_unwind_order(
        frames: &[MachineFrameSnapshot],
        scopes: &mut Vec<HandlerScopeId>,
        retries: &mut Vec<crate::orchestration::RetryAttemptRecord>,
    ) {
        for frame in frames.iter().rev() {
            for root in frame.continuation_roots().into_iter().flatten() {
                Self::continuation_unwind_order(root, scopes, retries);
            }
        }
    }

    fn handler_arm(&self, arm: &ActiveHandlerArmRecord, context: &str) -> Result<(), String> {
        let Some(expected) = self.checked.hir.handler_arms.get(arm.arm) else {
            return Err(format!(
                "{context} references a missing checked handler arm"
            ));
        };
        let Some(fact) = self.checked.types.handler_actions.get(&arm.arm) else {
            return Err(format!(
                "{context} is missing checked handler selector facts"
            ));
        };
        let expected_continuation =
            expected
                .continuation
                .and_then(|pat| match self.checked.hir.pats[pat] {
                    etas_hir::HirPat::Binding { symbol, .. } => Some(symbol),
                    _ => None,
                });
        if arm.body != expected.body
            || arm.scope != expected.scope
            || arm.patterns != expected.patterns
            || arm.continuation != expected_continuation
            || arm.action_symbol != Some(fact.action_symbol)
            || arm.action != expected.action.action
            || arm.span != expected.span
            || arm.effect_segments
                != expected
                    .action
                    .effect
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.name.clone())
                    .collect::<Vec<_>>()
        {
            return Err(format!("{context} does not match its checked handler arm"));
        }
        if let Some(symbol) = arm.action_symbol {
            self.symbol(symbol, &format!("{context} action symbol"))?;
        }
        for pat in &arm.patterns {
            self.pat(*pat, &format!("{context} pattern"))?;
        }
        if let Some(symbol) = arm.continuation {
            self.symbol(symbol, &format!("{context} continuation symbol"))?;
        }
        self.block(arm.body, &format!("{context} body"))?;
        self.scope(arm.scope, &format!("{context} scope"))
    }

    fn continuation_node(
        &self,
        continuation: &ContinuationSnapshot,
        context: &str,
        boundary_scopes: &mut BTreeSet<HandlerScopeId>,
    ) -> Result<(), String> {
        match continuation {
            ContinuationSnapshot::PackExistential { expr, frame } => {
                self.expr(*expr, context)?;
                if !self.checked.types.existential_packs.contains_key(expr) {
                    return Err("checkpoint packing continuation has no checked producer".into());
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::ResidualScope { scope } => {
                self.residual_references.borrow_mut().scopes.push(*scope);
                Ok(())
            }
            ContinuationSnapshot::ResidualOutcome { attempt } => {
                self.residual_references
                    .borrow_mut()
                    .attempts
                    .push(*attempt);
                Ok(())
            }
            ContinuationSnapshot::SourceToolReturn(frame) => {
                frame.validate_continuation()?;
                self.source_tool_return(frame, context, boundary_scopes)
            }
            ContinuationSnapshot::ProgramResume { id } => {
                self.suspension_references
                    .borrow_mut()
                    .push((*id, self.checked.clone()));
                Ok(())
            }
            ContinuationSnapshot::FileReadReduction { .. }
            | ContinuationSnapshot::DirectoryListReduction { .. } => {
                self.bounded_filesystem_reduction(continuation, context)
            }
            ContinuationSnapshot::StreamReadReduction { .. } => {
                self.stream_read_reduction(continuation, context)
            }
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
            ContinuationSnapshot::CalleeEval {
                call,
                args,
                span,
                frame,
            } => {
                if !matches!(
                    self.call_site(*call, args, *span, context)?,
                    etas_hir::HirExpr::Call { .. }
                ) {
                    return Err(format!(
                        "{context}: callee evaluation requires a checked call expression"
                    ));
                }
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
            | ContinuationSnapshot::RangeEnd { start: left, .. } => self.snapshot_value(left),
            ContinuationSnapshot::AggregateElement { .. }
            | ContinuationSnapshot::RecordField { .. }
            | ContinuationSnapshot::MapKey { .. }
            | ContinuationSnapshot::MapValue { .. } => {
                self.construction_continuation(continuation, context)
            }
            ContinuationSnapshot::IndexBase { .. }
            | ContinuationSnapshot::IndexValue { .. }
            | ContinuationSnapshot::SliceBase { .. }
            | ContinuationSnapshot::SliceStart { .. }
            | ContinuationSnapshot::SliceEnd { .. }
            | ContinuationSnapshot::PipelineInput { .. }
            | ContinuationSnapshot::PipelineTarget { .. }
            | ContinuationSnapshot::PipelineStageTarget { .. } => {
                self.access_continuation(continuation, context)
            }
            ContinuationSnapshot::MethodReceiver {
                expr,
                method,
                type_args,
                args,
                span,
                frame,
            } => {
                let site = self.call_site(*expr, args, *span, context)?;
                let (etas_hir::HirExpr::MethodCall {
                    method: expected, ..
                }
                | etas_hir::HirExpr::SpecMethodCall {
                    method: expected, ..
                }) = site
                else {
                    return Err(format!(
                        "{context}: receiver continuation is not a checked method call"
                    ));
                };
                let fact = self
                    .dispatch
                    .method(*expr)
                    .ok_or("checkpoint receiver has no prepared method target")?;
                if method != expected
                    || !type_args.is_empty()
                    || fact.receiver == etas_types::method::MethodReceiver::Associated
                {
                    return Err(format!(
                        "{context}: receiver continuation differs from its checked method selection"
                    ));
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::CallArgs { .. } => {
                self.pending_call_arguments(continuation, context)
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
                let Some(etas_hir::HirExpr::Perform {
                    action: checked_action,
                    args: checked_args,
                    ..
                }) = self.checked.hir.exprs.get(*expr)
                else {
                    return Err(format!(
                        "{context}: perform continuation does not reference a checked perform expression"
                    ));
                };
                let matches_args = args.len() == checked_args.len()
                    && args.iter().zip(checked_args).all(|(actual, expected)| {
                        match (actual, expected) {
                            (HirArg::Positional(a), HirArg::Positional(b)) => a == b,
                            (
                                HirArg::Named {
                                    name: a,
                                    value: av,
                                    span: asp,
                                },
                                HirArg::Named {
                                    name: b,
                                    value: bv,
                                    span: bsp,
                                },
                            ) => a == b && av == bv && asp == bsp,
                            _ => false,
                        }
                    });
                if action != checked_action || !matches_args {
                    return Err(format!(
                        "{context}: perform argument descriptors disagree with checked HIR"
                    ));
                }
                self.action(action, context)?;
                self.hir_types(type_args, context)?;
                self.args(args, context)?;
                self.index_at_most(*next_arg_index, args.len(), context)?;
                self.snapshot_values(evaluated_args)?;
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
            ContinuationSnapshot::ComposedCall {
                remaining,
                site,
                span,
            } => self.composed_continuation(remaining, site.as_ref(), *span, context),
            ContinuationSnapshot::CallBoundary { caller, span, .. } => {
                if let Some(caller) = caller {
                    self.call_origin(caller, *span, context)?;
                }
                Ok(())
            }
            ContinuationSnapshot::HandlerDispatch {
                deferred: Some(token),
                ..
            } => self
                .tasks
                .ok_or("deferred continuation requires its scheduler snapshot")?
                .validate_deferred(
                    *token,
                    self.task_identity
                        .ok_or("deferred continuation has no task identity")?,
                ),
            ContinuationSnapshot::RestoreModelPolicy { .. }
            | ContinuationSnapshot::HandlerDispatch { deferred: None, .. }
            | ContinuationSnapshot::ScopedModelPolicy { .. } => Ok(()),
            ContinuationSnapshot::ForLoop {
                pat,
                source,
                text_cursor,
                next_index,
                body,
                iterations,
                loop_scope,
                frame,
                ..
            } => {
                self.pat(*pat, context)?;
                self.block(*body, context)?;
                for symbol in loop_scope {
                    self.symbol(*symbol, context)?;
                }
                self.index_at_most(*next_index, *iterations, context)?;
                if *iterations == 0 || *iterations > u32::MAX as usize {
                    return Err(format!("{context} has an invalid for-loop iteration limit"));
                }
                if let Some(source) = source {
                    self.iteration_source(source, text_cursor.as_ref(), *next_index, context)?;
                } else if *next_index != 0 || text_cursor.is_some() {
                    return Err(format!(
                        "{context} has for-loop progress without an iteration source"
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
            ContinuationSnapshot::ReenterHandler { boundary } => {
                if !matches!(&**boundary, ContinuationSnapshot::HandleBoundary { .. }) {
                    return Err(format!(
                        "{context} handler reentry does not contain a handler boundary"
                    ));
                }
                Ok(())
            }
            ContinuationSnapshot::HandleBoundary { scope_id, .. } => {
                if scope_id.0 == u32::MAX {
                    return Err(format!("{context} handler scope identity is exhausted"));
                }
                if !boundary_scopes.insert(*scope_id) {
                    return Err(format!(
                        "{context} contains duplicate handle boundary scope {}",
                        scope_id.0
                    ));
                }
                Ok(())
            }
            ContinuationSnapshot::AgentInference { expr, .. } => {
                self.expr(*expr, context)?;
                if !matches!(
                    self.checked.hir.exprs.get(*expr),
                    Some(etas_hir::HirExpr::Infer { .. })
                ) || !self.checked.types.agent_inferences.contains_key(expr)
                {
                    return Err(format!(
                        "{context} inference continuation requires checked inference facts"
                    ));
                }
                Ok(())
            }
            ContinuationSnapshot::Chain { .. } => Ok(()),
        }
    }

    fn snapshot_value(&self, root: &ValueSnapshot) -> Result<(), String> {
        self.snapshot_value_in(root, call_target::ReferenceScope::NoCheckedSlot)
    }

    fn snapshot_value_in(
        &self,
        root: &ValueSnapshot,
        scope: call_target::ReferenceScope,
    ) -> Result<(), String> {
        // Iterator frames track depth, not the width of a collection.
        let mut query_seeds = Vec::new();
        for value in root.walk() {
            match value {
                ValueSnapshot::TaskRuntime(value) => self
                    .tasks
                    .ok_or("task value requires its scheduler snapshot")?
                    .validate_value(*value)?,
                ValueSnapshot::TextSequence {
                    source: _,
                    plan,
                    item_type,
                } => {
                    plan.validate().map_err(|error| {
                        format!("checkpoint has invalid text sequence: {error}")
                    })?;
                    self.type_id(*item_type, "checkpoint text sequence item")?;
                }
                ValueSnapshot::Tuple(values) if values.is_empty() => {
                    return Err("empty tuple must use the canonical unit value encoding".into());
                }
                ValueSnapshot::PriorityQueue {
                    comparator: Some(comparator),
                    ..
                } => {
                    let ValueSnapshot::Callable(target) = comparator.as_ref() else {
                        return Err("checkpoint priority comparison is not a callable".into());
                    };
                    self.collection_comparator(target, scope)?;
                }
                ValueSnapshot::Set(values) => {
                    crate::value::membership::MembershipIndex::require_unique(values)?;
                }
                ValueSnapshot::OrderedSet { values, comparator } => {
                    if let Some(value) = comparator {
                        let ValueSnapshot::Callable(target) = value.as_ref() else {
                            return Err("checkpoint key comparison is not a callable".into());
                        };
                        self.collection_comparator(target, scope)?;
                        continue;
                    }
                    for key in values.iter() {
                        crate::value::ordered::snapshot_order(key, key)?;
                    }
                    for pair in values.windows(2) {
                        if crate::value::ordered::snapshot_order(&pair[0], &pair[1])?
                            != std::cmp::Ordering::Less
                        {
                            return Err(
                                "checkpoint ordered set has duplicate or out-of-order keys".into(),
                            );
                        }
                    }
                }
                ValueSnapshot::OrderedMap {
                    entries,
                    comparator,
                } => {
                    if let Some(value) = comparator {
                        let ValueSnapshot::Callable(target) = value.as_ref() else {
                            return Err("checkpoint key comparison is not a callable".into());
                        };
                        self.collection_comparator(target, scope)?;
                        continue;
                    }
                    for (key, _) in entries.iter() {
                        crate::value::ordered::snapshot_order(key, key)?;
                    }
                    for pair in entries.windows(2) {
                        if crate::value::ordered::snapshot_order(&pair[0].0, &pair[1].0)?
                            != std::cmp::Ordering::Less
                        {
                            return Err(
                                "checkpoint ordered map has duplicate or out-of-order keys".into(),
                            );
                        }
                    }
                }
                ValueSnapshot::MemoryWriteIntent(value) => {
                    value.validate(self.checked, self.limits)?
                }
                ValueSnapshot::Schema(ty) => {
                    self.type_id(*ty, "checkpoint schema descriptor type")?
                }
                ValueSnapshot::StdConstant(value) => {
                    self.type_id(value.ty(), "checkpoint standard constant type")?;
                    value.validate(&self.checked.types.known_std_types)?;
                }
                ValueSnapshot::CodeIdentity { ty, identity } => {
                    self.type_id(*ty, "checkpoint compiler identity type")?;
                    identity.restore(&self.checked.types.known_std_types.code, *ty)?;
                }
                ValueSnapshot::Nominal { ty, .. } => {
                    self.type_id(*ty, "checkpoint nominal value type")?
                }
                ValueSnapshot::Existential { header, value } => {
                    let bindings = header.validate(self.checked)?;
                    self.reference_value(value, header.concrete_type, &bindings)?;
                }
                ValueSnapshot::Conversation(conversation) => {
                    crate::value::conversation::validate_snapshot(conversation, self.limits)?
                }
                ValueSnapshot::Callable(target) => {
                    self.call_target_in(target, "checkpoint callable value", scope)?
                }
                ValueSnapshot::Handler {
                    fact_expr,
                    handlers,
                } => {
                    self.expr(*fact_expr, "checkpoint handler value fact expression")?;
                    for (index, handler) in handlers.iter().enumerate() {
                        self.handler_arm(
                            handler,
                            &format!("checkpoint handler value arm {index}"),
                        )?;
                    }
                }
                ValueSnapshot::ResourceHandle { ty, .. } => {
                    self.type_id(*ty, "checkpoint resource handle type")?
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
                        kind,
                        predicate_type,
                        predicate,
                        ..
                    } = value
                    {
                        query_seeds.extend(crate::value::contract::selection_query(
                            kind,
                            *predicate_type,
                            predicate
                                .as_deref()
                                .map(crate::value::contract::Value::Snapshot),
                        )?);
                    }
                }
                _ => {}
            }
        }
        crate::value::contract::validate(self, query_seeds, &crate::control::GenericBindings::new())
    }

    fn snapshot_values(&self, values: &[ValueSnapshot]) -> Result<(), String> {
        for value in values {
            self.snapshot_value(value)?;
        }
        Ok(())
    }

    fn iteration_source(
        &self,
        source: &ValueSnapshot,
        text_cursor: Option<&etas_builtin::text::iteration::TextCursorState>,
        position: usize,
        context: &str,
    ) -> Result<(), String> {
        match source {
            ValueSnapshot::TextSequence {
                source,
                plan,
                item_type,
            } => {
                self.type_id(*item_type, context)?;
                plan.validate()
                    .map_err(|error| format!("{context} has invalid text sequence: {error}"))?;
                let cursor = text_cursor
                    .ok_or_else(|| format!("{context} is missing its text iteration cursor"))?;
                if cursor.byte_offset > source.len() || !source.is_char_boundary(cursor.byte_offset)
                {
                    return Err(format!("{context} has an invalid text cursor boundary"));
                }
                let mut expected = etas_builtin::text::iteration::TextCursorState::default();
                for _ in 0..position {
                    if matches!(
                        etas_builtin::text::iteration::step(plan, &mut expected, source.as_str()),
                        Ok(etas_builtin::text::iteration::TextSequenceStep::End) | Err(_)
                    ) {
                        return Err(format!(
                            "{context} has a text iteration position past the sequence end"
                        ));
                    }
                }
                if &expected != cursor {
                    return Err(format!(
                        "{context} text cursor does not match its completed iteration count"
                    ));
                }
            }
            ValueSnapshot::Array(values)
            | ValueSnapshot::List(values)
            | ValueSnapshot::Slice(values)
            | ValueSnapshot::Set(values)
            | ValueSnapshot::Deque(values)
            | ValueSnapshot::Queue(values)
            | ValueSnapshot::Stack(values)
            | ValueSnapshot::OrderedSet { values, .. } => {
                self.index_at_most(position, values.len(), context)?
            }
            ValueSnapshot::Map(values)
            | ValueSnapshot::PriorityQueue {
                entries: values, ..
            }
            | ValueSnapshot::OrderedMap {
                entries: values, ..
            } => self.index_at_most(position, values.len(), context)?,
            ValueSnapshot::Range { start, end, bounds } => {
                let (ValueSnapshot::Number(start), ValueSnapshot::Number(end)) = (&**start, &**end)
                else {
                    return Err(format!("{context} has non-integer range bounds"));
                };
                let valid = crate::value::range::IntegerRange::new(*start, *end, *bounds)
                    .and_then(|range| range.allows_position(position))
                    .map_err(|error| format!("{context} has invalid range bounds: {error:?}"))?;
                if !valid {
                    return Err(format!(
                        "{context} has a for-loop position outside its range"
                    ));
                }
            }
            _ => return Err(format!("{context} has a non-iterable for-loop source")),
        }
        if !matches!(source, ValueSnapshot::TextSequence { .. }) && text_cursor.is_some() {
            return Err(format!(
                "{context} has a text cursor for a non-text iteration source"
            ));
        }
        self.snapshot_value(source)
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
                HirEffectArg::PatternBinding { .. } => {
                    return Err(format!(
                        "{context}: trace pattern binding is not a runtime action selector"
                    ));
                }
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

    fn record_construction(
        &self,
        expr: HirExprId,
        ty: Option<TypeId>,
        variant: Option<SymbolId>,
        context: &str,
    ) -> Result<&[HirFieldInit], String> {
        let error = || {
            format!(
                "{context}: record or named enum continuation does not match its checked construction"
            )
        };
        let Some(etas_hir::HirExpr::Record(record)) = self.checked.hir.exprs.get(expr) else {
            return Err(error());
        };
        let expected_type = if record.path.is_some() {
            Some(
                self.checked
                    .types
                    .expression_producer_type(expr)
                    .ok_or_else(error)?,
            )
        } else {
            None
        };
        let expected_variant = match record.path.as_ref().map(|path| &path.resolution) {
            Some(etas_hir::ResolveResult::Resolved(symbol)) => {
                let symbol = self.checked.symbols.get(*symbol).ok_or_else(error)?;
                self.checked
                    .types
                    .enum_constructor(&self.checked.type_store, symbol)
                    .map(|_| symbol.id)
            }
            None => None,
            _ => return Err(error()),
        };
        if ty != expected_type || variant != expected_variant {
            return Err(error());
        }
        if expected_variant.is_none()
            && let Some(ty) = ty
        {
            let base = match self.checked.type_store.get(ty) {
                Some(etas_types::Type::Applied { constructor, .. }) => TypeId(constructor.0),
                _ => ty,
            };
            if matches!(
                self.checked.type_store.get(base),
                Some(etas_types::Type::Enum(_))
            ) {
                return Err(error());
            }
        }
        Ok(&record.fields)
    }

    fn named_variant_fields(
        &self,
        symbol: SymbolId,
        ty: Option<TypeId>,
        fields: &[HirFieldInit],
        context: &str,
    ) -> Result<(), String> {
        let error =
            || format!("{context}: named enum continuation does not match its checked constructor");
        let symbol = self.checked.symbols.get(symbol).ok_or_else(error)?;
        let (owner, _, variant) = self
            .checked
            .types
            .enum_constructor(&self.checked.type_store, symbol)
            .ok_or_else(error)?;
        let names = variant.field_names.as_ref().ok_or_else(error)?;
        let ty = ty.ok_or_else(error)?;
        let base = match self.checked.type_store.get(ty) {
            Some(etas_types::Type::Applied { constructor, .. }) => TypeId(constructor.0),
            Some(etas_types::Type::Enum(_)) => ty,
            _ => return Err(error()),
        };
        if base != owner || fields.len() != names.len() {
            return Err(error());
        }
        let actual = fields
            .iter()
            .map(|field| match field {
                HirFieldInit::Named { name, .. } | HirFieldInit::Shorthand { name, .. } => name,
            })
            .collect::<BTreeSet<_>>();
        if actual.len() != fields.len() || actual != names.iter().collect() {
            return Err(error());
        }
        Ok(())
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
    use etas_hir::{HirBlockId, HirExprId, HirItemId, HirPatId, ScopeId};

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
            residual_invocation: None,
            effect_row_bindings: Vec::new(),
            id: 1,
            locals: Default::default(),
            type_parameter_bindings: Vec::new(),
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
        let closures = crate::plan::ClosureLayoutTable::build(checked, &slots)?;
        SnapshotValidator::new(
            checked,
            &slots,
            &dispatch,
            &closures,
            &etas_host::StorageLimits::default(),
        )
        .validate_machine(&MachineSnapshot {
            auxiliary: Vec::new(),
            input: None,
            program_owners: Default::default(),
            suspended_programs: Vec::new(),
            frames: vec![MachineFrameSnapshot::Continuation { continuation }],
            residual: Default::default(),
        })
    }

    #[test]
    fn frame_definition_validation_does_not_copy_the_captured_value_graph() {
        use crate::{
            control::Frame,
            testing::allocation::measure,
            value::{ArrayValue, InterpValue},
        };
        let checked = crate::testing::project::checked_project(
            "module app.main; flow main() -> unit { let values: Array<string> = []; return; }",
        );
        let plan = crate::Interpreter
            .plan(&checked, crate::api::PlanOptions)
            .plan
            .unwrap();
        let symbol = checked
            .symbols
            .iter()
            .find(|s| s.name == "values")
            .unwrap()
            .id;
        let block = checked.hir.blocks.iter().next().unwrap().0;
        let limits = etas_host::StorageLimits::default();
        let mut baseline = None;
        for count in [1000, 2000, 4000] {
            let frame = Frame::from_snapshot(vec![(
                symbol,
                InterpValue::Array(ArrayValue::new(
                    (0..count)
                        .map(|_| InterpValue::String("payload".repeat(128).into()))
                        .collect(),
                )),
            )])
            .unwrap();
            let frame = super::super::frame::capture_frame(&frame).unwrap();
            let machine = MachineSnapshot {
                auxiliary: Vec::new(),
                input: None,
                program_owners: Default::default(),
                suspended_programs: Vec::new(),
                frames: (0..3)
                    .map(|_| MachineFrameSnapshot::Continuation {
                        continuation: ContinuationSnapshot::ContinueBlock {
                            block,
                            next_stmt_index: 0,
                            frame: frame.clone(),
                        },
                    })
                    .collect(),
                residual: Default::default(),
            };
            let validator = SnapshotValidator::new(
                &checked,
                &plan.slots,
                &plan.dispatch,
                &plan.closures,
                &limits,
            );
            let (result, cost) = measure(|| validator.validate_machine(&machine));
            result.unwrap();
            assert!(
                cost.bytes < 4096,
                "only validation bookkeeping, n={count}: {cost:?}"
            );
            let measured = (cost.count, cost.bytes);
            assert_eq!(*baseline.get_or_insert(measured), measured, "n={count}");
        }
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
                    expr: HirExprId(u32::MAX),
                    method: "method".to_owned(),
                    type_args: Vec::new(),
                    args: Vec::new(),
                    span: span(),
                    frame: empty_locals(),
                },
                "missing checked call site",
            ),
            (
                ContinuationSnapshot::CallArgs {
                    call: expr,
                    target: CallTargetSnapshot::ToolItem(HirItemId(u32::MAX)),
                    args: Vec::new().into(),
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
        let checked = crate::testing::project::checked_project(
            r#"module app.main;
effect Gate { action request() -> unit; }
flow main() -> unit { perform Gate.request() with { Gate.request() => resume; }; }
"#,
        );
        let (arm, handler) = checked.hir.handler_arms.iter().next().expect("handler arm");
        let slots = SlotLayoutTable::for_project(&checked);
        let dispatch = crate::plan::IntrinsicDispatchTable::for_project(&checked)
            .expect("test intrinsic dispatch");
        let limits = etas_host::StorageLimits::default();
        let closures = crate::plan::ClosureLayoutTable::build(&checked, &slots).unwrap();
        let validator = SnapshotValidator::new(&checked, &slots, &dispatch, &closures, &limits);
        let error = validator
            .handler_arm(
                &ActiveHandlerArmRecord {
                    arm,
                    effect_segments: vec!["Gate".to_owned()],
                    action: "request".to_owned(),
                    action_symbol: Some(checked.types.handler_actions[&arm].action_symbol),
                    patterns: handler.patterns.clone(),
                    continuation: None,
                    body: handler.body,
                    scope: ScopeId(u32::MAX),
                    span: handler.span,
                },
                "test handler",
            )
            .expect_err("invalid handler scope must fail closed");
        assert!(error.contains("does not match its checked handler arm"));
    }
}

#[derive(Default)]
struct ResidualReferences {
    scopes: Vec<u32>,
    attempts: Vec<u32>,
    owners: std::collections::HashMap<u32, (usize, HirItemId, u64)>,
}
