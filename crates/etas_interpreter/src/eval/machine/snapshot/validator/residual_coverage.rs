use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;

use etas_hir::{HirBodyKind, HirItemId, HirNodeRef, HirOwner};

use super::{ContinuationSnapshot, InterpreterCheckpoint, MachineFrameSnapshot, SnapshotValidator};
use crate::orchestration::{ContinuationSnapshotLink, LocalsSnapshot, ResidualMachineSnapshot};

type Scope = Option<u32>;
type SharedKey = (NonNull<ContinuationSnapshot>, Scope);

enum Task<'a> {
    Node(&'a ContinuationSnapshot),
    Link(&'a ContinuationSnapshotLink),
    Complete(SharedKey),
    Restore(Scope),
}

/// Reconstructs scope ownership in reverse unwind order. Captured handler
/// continuations are checked in their own branch, not made active in the arm.
struct Coverage<'a, 'checked> {
    validator: &'a SnapshotValidator<'checked>,
    residual: &'a ResidualMachineSnapshot,
    owners: HashMap<HirOwner, Option<HirItemId>>,
    shared: HashMap<SharedKey, Scope>,
}

impl SnapshotValidator<'_> {
    pub(super) fn validate_root_residual_coverage(
        &self,
        checkpoint: &InterpreterCheckpoint,
    ) -> Result<(), String> {
        let item = checkpoint.entry_item;
        // An agent body can already have returned before its model loop runs.
        // A source flow's root invocation remains live until the run returns.
        if !matches!(
            self.checked.hir.items.get(item),
            Some(etas_hir::HirItem::Flow(_))
        ) || self
            .checked
            .residual_plans()
            .item_plans(item)
            .next()
            .is_none()
        {
            return Ok(());
        }
        for frame in &checkpoint.machine.frames {
            match frame {
                MachineFrameSnapshot::ResidualScope { scope } => {
                    return match checkpoint.machine.residual.scopes.get(*scope as usize) {
                        Some(scope) if scope.item == item.0 => Ok(()),
                        _ => Err("checkpoint is missing the root's required residual scope".into()),
                    };
                }
                MachineFrameSnapshot::Call { .. } | MachineFrameSnapshot::ProgramReturn(_) => break,
                _ => {}
            }
        }
        Err("checkpoint is missing the root's required residual scope".into())
    }

    pub(super) fn validate_residual_frame_coverage(
        &self,
        frames: &[MachineFrameSnapshot],
        residual: &ResidualMachineSnapshot,
        pending: Option<&ContinuationSnapshot>,
    ) -> Result<(), String> {
        if self.checked.residual_plans().is_empty() {
            return Ok(());
        }
        let mut coverage = Coverage {
            validator: self,
            residual,
            owners: HashMap::new(),
            shared: HashMap::new(),
        };
        let mut scope = None;
        for frame in frames {
            if let MachineFrameSnapshot::ResidualScope { scope: index } = frame {
                coverage.scope(*index)?;
                scope = Some(*index);
            } else {
                if let MachineFrameSnapshot::ProgramReturn(boundary) = frame {
                    coverage.location(
                        call_site_node(boundary.call_site.kind),
                        &boundary.call_site.frame,
                        scope,
                    )?;
                }
                if let MachineFrameSnapshot::Call {
                    caller: Some(caller),
                    ..
                } = frame
                {
                    coverage.origin(caller, scope)?;
                }
                for root in frame.continuation_roots().into_iter().flatten() {
                    coverage.continuation(root, scope)?;
                }
            }
        }
        if let Some(pending) = pending {
            coverage.continuation(pending, scope)?;
        }
        Ok(())
    }
}

impl Coverage<'_, '_> {
    fn origin(&mut self, origin: &crate::control::CallOrigin, scope: Scope) -> Result<(), String> {
        let HirNodeRef::Expr(expr) = call_site_node(origin.kind) else {
            unreachable!()
        };
        let Some(item) = self.required_owner(HirOwner::Expr(expr))? else {
            return Ok(());
        };
        let index = scope.ok_or("checkpoint caller is missing its required residual scope")?;
        let invocation = crate::eval::residual::invocation_counter(
            &self.residual.scopes[index as usize].invocation,
        )?;
        if self.scope(index)? != item || origin.residual_invocation != Some(invocation) {
            return Err("checkpoint caller has another required residual scope invocation".into());
        }
        Ok(())
    }

    fn scope(&self, index: u32) -> Result<HirItemId, String> {
        let scope = self
            .residual
            .scopes
            .get(index as usize)
            .ok_or("checkpoint references an unknown residual scope")?;
        let item = HirItemId(scope.item);
        let revision = self
            .validator
            .checked
            .residual_plans()
            .binding()
            .and_then(|binding| binding.revision(item))
            .ok_or("checkpoint required residual scope has no checked owner")?;
        if revision.key() != scope.revision {
            return Err("checkpoint required residual scope belongs to another revision".into());
        }
        Ok(item)
    }

    fn continuation(
        &mut self,
        root: &ContinuationSnapshot,
        mut scope: Scope,
    ) -> Result<(), String> {
        let mut pending = vec![Task::Node(root)];
        while let Some(task) = pending.pop() {
            let node = match task {
                Task::Complete(key) => {
                    self.shared.insert(key, scope);
                    continue;
                }
                Task::Restore(previous) => {
                    scope = previous;
                    continue;
                }
                Task::Link(link) => {
                    if let Some(identity) = link.shared_identity() {
                        let key = (identity, scope);
                        if let Some(previous) = self.shared.get(&key) {
                            scope = *previous;
                            continue;
                        }
                        pending.push(Task::Complete(key));
                    }
                    &**link
                }
                Task::Node(node) => node,
            };
            if let Some((location, frame)) = location(node) {
                self.location(location, frame, scope)?;
            }
            if let ContinuationSnapshot::CallBoundary {
                caller: Some(caller),
                ..
            } = node
            {
                self.origin(caller, scope)?;
            }
            match node {
                ContinuationSnapshot::ResidualScope { scope: index } => {
                    self.scope(*index)?;
                    scope = Some(*index);
                }
                ContinuationSnapshot::Chain { inner, outer } => {
                    // Outer continuations unwind later: their scopes enclose inner.
                    pending.push(Task::Link(inner));
                    pending.push(Task::Link(outer));
                }
                ContinuationSnapshot::HandlerDispatch { outer } => {
                    pending.push(Task::Restore(scope));
                    pending.push(Task::Link(outer));
                }
                ContinuationSnapshot::SourceToolReturn(saved) => {
                    pending.push(Task::Node(&saved.model_loop.pending.continuation));
                    pending.push(Task::Node(&saved.model_loop.outer_continuation));
                }
                ContinuationSnapshot::RestoreModelPolicy { inner, .. }
                | ContinuationSnapshot::ScopedModelPolicy { inner, .. }
                | ContinuationSnapshot::HandleBoundary { inner, .. }
                | ContinuationSnapshot::ReenterHandler { boundary: inner }
                | ContinuationSnapshot::CallBoundary { outer: inner, .. } => {
                    pending.push(Task::Link(inner));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn location(
        &mut self,
        node: HirNodeRef,
        frame: &LocalsSnapshot,
        scope: Scope,
    ) -> Result<(), String> {
        let owner = match node {
            HirNodeRef::Block(block) => HirOwner::Block(block),
            HirNodeRef::Expr(expr) => HirOwner::Expr(expr),
            HirNodeRef::Pat(pat) => HirOwner::Pattern(pat),
            _ => return Err("checkpoint residual location has no executable HIR owner".into()),
        };
        let Some(item) = self.required_owner(owner)? else {
            return Ok(());
        };
        let index =
            scope.ok_or("checkpoint continuation is missing its required residual scope")?;
        if self.scope(index)? != item {
            return Err(
                "checkpoint continuation has another callable's required residual scope".into(),
            );
        }
        let expected_invocation = crate::eval::residual::invocation_counter(
            &self.residual.scopes[index as usize].invocation,
        )?;
        if frame.residual_invocation != Some(expected_invocation) {
            return Err(
                "checkpoint frame does not own its required residual scope invocation".into(),
            );
        }
        let selected = self.residual.scopes[index as usize].application;
        let checked = self.validator.checked;
        let applications = checked
            .residual_plans()
            .applications(checked, item)
            .map_err(|error| format!("checkpoint residual application is invalid: {error:?}"))?;
        if !applications.iter().any(|application| {
            application.fingerprint == selected
                && application
                    .type_bindings
                    .iter()
                    .all(|binding| frame.type_bindings.contains(binding))
                && application
                    .effect_row_bindings
                    .iter()
                    .all(|binding| frame.effect_row_bindings.contains(binding))
        }) {
            return Err("checkpoint frame has different residual generic bindings".into());
        }
        let owner = (
            std::sync::Arc::as_ptr(self.validator.checked) as usize,
            item,
            frame.id,
        );
        if self
            .validator
            .residual_references
            .borrow_mut()
            .owners
            .insert(index, owner)
            .is_some_and(|previous| previous != owner)
        {
            return Err("checkpoint invocations share one required residual scope".into());
        }
        Ok(())
    }

    fn required_owner(&mut self, mut owner: HirOwner) -> Result<Option<HirItemId>, String> {
        let mut path = Vec::new();
        let mut seen = HashSet::new();
        let index = self.validator.checked.hir.index();
        let result = loop {
            if let Some(item) = self.owners.get(&owner) {
                break *item;
            }
            if !seen.insert(owner) {
                return Err("checkpoint location has a cyclic checked HIR owner".into());
            }
            path.push(owner);
            let parent = match owner {
                HirOwner::Item(item) => break Some(item),
                HirOwner::Module(_) => break None,
                HirOwner::Body(body) => match body.kind {
                    HirBodyKind::Flow
                    | HirBodyKind::Tool
                    | HirBodyKind::Agent
                    | HirBodyKind::ImplFlow(_) => break Some(body.item),
                    HirBodyKind::Lambda(_) | HirBodyKind::HandlerArm(_) => break None,
                    _ => index.body_owner.get(&body).copied(),
                },
                HirOwner::Block(block) => index.block_owner.get(&block).copied(),
                HirOwner::Stmt(stmt) => index.stmt_block.get(&stmt).copied().map(HirOwner::Block),
                HirOwner::Expr(expr) => index.expr_owner.get(&expr).copied(),
                HirOwner::HandlerArm(_) => break None,
                HirOwner::Pattern(pat) => index.pat_owner.get(&pat).copied(),
            };
            owner = parent.ok_or("checkpoint location has no checked HIR owner")?;
        };
        let required = result.filter(|item| {
            self.validator
                .checked
                .residual_plans()
                .item_plans(*item)
                .next()
                .is_some()
        });
        for owner in path {
            self.owners.insert(owner, required);
        }
        Ok(required)
    }
}

fn call_site_node(site: crate::control::CallSiteKind) -> HirNodeRef {
    let (crate::control::CallSiteKind::Expr { expr }
    | crate::control::CallSiteKind::Pipeline { expr }
    | crate::control::CallSiteKind::PipelineStage { expr, .. }) = site;
    HirNodeRef::Expr(expr)
}

fn location(node: &ContinuationSnapshot) -> Option<(HirNodeRef, &LocalsSnapshot)> {
    use ContinuationSnapshot::*;
    let (node, frame) = match node {
        ContinueBlock { block, frame, .. }
        | Bind { block, frame, .. }
        | Assign { block, frame, .. }
        | AssignTargetIndex { block, frame, .. }
        | IfStmt { block, frame, .. }
        | MatchStmt { block, frame, .. }
        | RetryAttempt { block, frame, .. } => (HirNodeRef::Block(*block), frame),
        IfExpr {
            then_block, frame, ..
        } => (HirNodeRef::Block(*then_block), frame),
        ForLoop { body, frame, .. } | WhileLoop { body, frame, .. } => {
            (HirNodeRef::Block(*body), frame)
        }
        FieldReceiver { expr, frame, .. }
        | AggregateElement { expr, frame, .. }
        | RecordField { expr, frame, .. }
        | MapKey { expr, frame, .. }
        | MapValue { expr, frame, .. }
        | IndexBase { expr, frame, .. }
        | IndexValue { expr, frame, .. }
        | MethodReceiver { expr, frame, .. }
        | LocalMethodArgs { expr, frame, .. }
        | StaticMethodArgs { expr, frame, .. }
        | SpecMethodReceiver { expr, frame, .. }
        | PipelineStageTarget { expr, frame, .. }
        | PerformArgs { expr, frame, .. }
        | PipelineInput { expr, frame, .. }
        | PipelineTarget { expr, frame, .. } => (HirNodeRef::Expr(*expr), frame),
        BinaryLeft { rhs, frame, .. }
        | ListConsHead {
            tail: rhs, frame, ..
        }
        | RangeStart {
            end: rhs, frame, ..
        } => (HirNodeRef::Expr(*rhs), frame),
        CalleeEval { call, frame, .. } | CallArgs { call, frame, .. } => {
            (HirNodeRef::Expr(*call), frame)
        }
        SliceBase { eval, frame }
        | SliceStart { eval, frame, .. }
        | SliceEnd { eval, frame, .. } => (HirNodeRef::Expr(eval.expr), frame),
        HandleHandler {
            handle_expr, frame, ..
        } => (HirNodeRef::Expr(*handle_expr), frame),
        MatchExpr { arms, frame, .. } => (HirNodeRef::Pat(arms.first()?.pat), frame),
        VariantArgs { args, frame, .. }
        | MemoryArgs { args, frame, .. }
        | MemorySelectionLimitArgs { args, frame, .. } => {
            let (etas_hir::HirArg::Positional(expr) | etas_hir::HirArg::Named { value: expr, .. }) =
                args.first()?;
            (HirNodeRef::Expr(*expr), frame)
        }
        ComposedCall {
            site: Some(site), ..
        } => (call_site_node(site.kind), &site.frame),
        ProgramResume { .. }
        | SourceToolReturn(_)
        | ReenterHandler { .. }
        | FileReadReduction { .. }
        | ResidualScope { .. }
        | ResidualOutcome { .. }
        | StreamReadReduction { .. }
        | Unary { .. }
        | BinaryRight { .. }
        | ListConsTail { .. }
        | RangeEnd { .. }
        | PromptValueMethodArg { .. }
        | ComposedCall { site: None, .. }
        | RestoreModelPolicy { .. }
        | CallBoundary { .. }
        | TryExpr { .. }
        | MemoryClearDeleteAll { .. }
        | MemoryClearDeleteNext { .. }
        | HandlerDispatch { .. }
        | HandleBoundary { .. }
        | AgentInference { .. }
        | ScopedModelPolicy { .. }
        | Chain { .. }
        | Return
        | Resume
        | Finish
        | BlockValue => return None,
    };
    Some((node, frame))
}
