use std::rc::Rc;

use etas_core::{AnalysisDiagnosticCode, Span};

use super::ResidualMirrorEntry;
use crate::control::{
    Continuation, ExecutionFault, PendingPerform,
    residual::{ResidualAttemptTag, ResidualObservedScope, SharedResidualScope},
};
use crate::eval::{EvalContext, compose_continuation};
use crate::value::InterpValue;

fn violation(span: Span, reason: impl Into<String>) -> ExecutionFault {
    ExecutionFault::new(
        AnalysisDiagnosticCode::TraceSpecResidualViolation,
        span,
        reason,
    )
}

impl EvalContext<'_> {
    /// Observes a governed action at its first dispatch point. Every scope
    /// that dynamically encloses the action previews the same attempt; the
    /// action is rejected before any handler, default implementation or Host
    /// boundary runs unless every scope permits it. Permitted transitions are
    /// committed together and the attempt's outcome is correlated through a
    /// continuation installed here. Idempotent for an observed action.
    pub(crate) fn observe_residual_attempt(
        &mut self,
        perform: &mut PendingPerform,
    ) -> Result<(), ExecutionFault> {
        if perform.error_type.is_some() || perform.residual.is_some() {
            return Ok(());
        }
        let span = perform.span;
        let mut captured = Vec::new();
        perform.continuation.residual_scopes(&mut captured);
        let mirror = self.task.residual.mirror.clone();
        let mirror = mirror.borrow();
        if captured.is_empty() && !mirror.has_scopes() {
            return Ok(());
        }
        let Some(mut proof) = perform.checked_action.clone() else {
            return Err(violation(
                span,
                "governed action has no checked invocation for residual observation",
            ));
        };
        if !std::sync::Arc::ptr_eq(&proof.program, self.checked) {
            return Err(violation(
                span,
                "governed action invocation belongs to a different checked program",
            ));
        }
        let mut scopes: Vec<(SharedResidualScope, etas_effects::Effect)> = Vec::new();
        let mut include = |scope: &SharedResidualScope,
                           proof: &crate::control::CheckedActionInvocation|
         -> Result<(), ExecutionFault> {
            if scopes.iter().any(|(seen, _)| Rc::ptr_eq(seen, scope)) {
                return Ok(());
            }
            if !std::sync::Arc::ptr_eq(scope.borrow().program(), &proof.program) {
                return Err(violation(
                    span,
                    "residual scope and governed action disagree on their checked program",
                ));
            }
            scopes.push((scope.clone(), proof.effect.clone()));
            Ok(())
        };
        for scope in &captured {
            include(scope, &proof)?;
        }
        let mut boundaries = Vec::new();
        for entry in mirror.top_down() {
            match entry {
                ResidualMirrorEntry::Boundary(boundary) => boundaries.push(boundary),
                ResidualMirrorEntry::Scope(scope) => {
                    for boundary in boundaries.drain(..) {
                        let boundary = boundary
                            .as_ref()
                            .map_err(|reason| violation(span, reason.clone()))?;
                        proof = crate::plan::code::ActionProjection::new(
                            proof,
                            boundary.projections.clone(),
                            &boundary.parent,
                        )
                        .map_err(|reason| violation(span, reason))?
                        .target()
                        .clone();
                    }
                    include(scope, &proof)?;
                }
            }
        }
        drop(mirror);
        let id = self
            .task
            .residual
            .next_attempt()
            .map_err(|reason| violation(span, reason))?;
        let cursor = self
            .task
            .residual
            .next_cursor()
            .map_err(|reason| violation(span, reason))?;
        let mut previews = Vec::with_capacity(scopes.len());
        for (scope, effect) in scopes {
            let label = scope
                .try_borrow_mut()
                .map_err(|_| violation(span, "residual scope is busy"))?
                .label(&effect)
                .map_err(|reason| violation(span, reason))?;
            let preview = scope
                .try_borrow()
                .map_err(|_| violation(span, "residual scope is busy"))?
                .preview(id, cursor, &label)
                .map_err(|reason| violation(span, reason))?;
            match preview {
                crate::control::residual::ResidualPreview::Permit(deltas) => {
                    previews.push((scope.clone(), label, deltas))
                }
                crate::control::residual::ResidualPreview::Reject => {
                    return Err(violation(
                        span,
                        format!(
                            "trace spec residual check rejected action `{}.{}` before dispatch",
                            crate::eval::path_name(&perform.action),
                            perform.action.action
                        ),
                    ));
                }
                crate::control::residual::ResidualPreview::NeedsEvidence => {
                    return Err(violation(
                        span,
                        format!(
                            "trace spec residual check for action `{}.{}` requires prior evidence that is still pending",
                            crate::eval::path_name(&perform.action),
                            perform.action.action
                        ),
                    ));
                }
            }
        }
        let mut observed = Vec::with_capacity(previews.len());
        for (scope, label, deltas) in previews {
            let version = scope
                .try_borrow_mut()
                .map_err(|_| violation(span, "residual scope is busy"))?
                .commit(deltas, id, cursor, label.clone())
                .map_err(|reason| violation(span, reason))?;
            observed.push(ResidualObservedScope {
                instance: scope,
                action: label,
                version,
            });
        }
        let decision = crate::control::residual::is_standard_approval(
            &self.checked.effect_registry,
            &proof.effect,
        );
        let tag = ResidualAttemptTag::new(id, cursor, observed, decision);
        perform.residual = Some(tag.clone());
        perform.continuation = compose_continuation(
            Continuation::ResidualOutcome(tag),
            std::mem::replace(&mut perform.continuation, Continuation::BlockValue),
        );
        Ok(())
    }

    /// Consumes the single-dispatch permit of an observed action immediately
    /// before its handler, default implementation or Host boundary runs.
    pub(crate) fn consume_residual_permit(
        &self,
        perform: &PendingPerform,
    ) -> Result<(), ExecutionFault> {
        match &perform.residual {
            Some(tag) => tag
                .consume_permit()
                .map_err(|reason| violation(perform.span, reason)),
            None => Ok(()),
        }
    }

    /// Delivers a completed action outcome to the scopes that committed it.
    pub(crate) fn complete_residual_attempt(
        &mut self,
        tag: &ResidualAttemptTag,
        value: &InterpValue,
        span: Span,
    ) -> Result<(), ExecutionFault> {
        let cursor = self
            .task
            .residual
            .next_cursor()
            .map_err(|reason| violation(span, reason))?;
        tag.complete(value, cursor)
            .map_err(|reason| violation(span, reason))
    }
}
