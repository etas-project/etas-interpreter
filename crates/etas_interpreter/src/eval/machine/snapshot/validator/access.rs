use super::*;
use etas_hir::HirExpr;

impl SnapshotValidator<'_> {
    pub(super) fn composed_continuation(
        &self,
        remaining: &[CallTargetSnapshot],
        site: Option<&crate::orchestration::CallSiteSnapshot>,
        span: etas_core::Span,
        context: &str,
    ) -> Result<(), String> {
        if let Some(site) = site {
            if site.composition.is_some() {
                self.invocation_site(site, span, context)?;
                let composition = site
                    .composition
                    .as_ref()
                    .ok_or("checkpoint composition proof is missing")?;
                let (stages, index) = composition.stages_and_index()?;
                if &stages[index..] != remaining {
                    return Err(
                        "checkpoint composition invocation has an inconsistent remaining suffix"
                            .into(),
                    );
                }
                return Ok(());
            }
            self.frame(&site.frame, context)?;
            if let crate::control::CallSiteKind::PipelineStage { expr, index } = site.kind {
                let stages =
                    crate::eval::pipeline::checked_pipeline_stages(self.checked, expr, span)?;
                let suffix = stages
                    .get(index..)
                    .ok_or("checkpoint pipeline invocation cursor is out of bounds")?;
                if suffix.len() != remaining.len() {
                    return Err(
                        "checkpoint pipeline invocation has an inconsistent remaining suffix"
                            .into(),
                    );
                }
                let bindings = site.frame.generic_bindings();
                for (target, stage) in remaining.iter().zip(suffix) {
                    self.call_target_in(target, context, call_target::ReferenceScope::CheckedSlot)?;
                    let ty = *self
                        .checked
                        .types
                        .expr_types
                        .get(&stage.expr)
                        .ok_or("checkpoint pipeline stage has no checked type")?;
                    self.reference_target_type_with_bindings(target, ty, &bindings)?;
                }
                return Ok(());
            }
            site.receiver(self.checked, span)?;
        }
        for target in remaining {
            self.call_target(target, context)?;
        }
        Ok(())
    }

    pub(super) fn access_continuation(
        &self,
        state: &ContinuationSnapshot,
        context: &str,
    ) -> Result<(), String> {
        match state {
            ContinuationSnapshot::IndexBase {
                expr,
                index,
                span,
                frame,
            } => {
                let (_, expected) = self.checked_index(*expr, *span)?;
                if *index != expected {
                    return Err("checkpoint index operand differs from its checked owner".into());
                }
                self.frame(frame, context)
            }
            ContinuationSnapshot::IndexValue {
                expr,
                base,
                span,
                frame,
            } => {
                let (operand, _) = self.checked_index(*expr, *span)?;
                self.frame(frame, context)?;
                let bindings = frame.generic_bindings();
                self.typed_expression_value(base, operand, &bindings)
            }
            ContinuationSnapshot::SliceBase { eval, frame }
            | ContinuationSnapshot::SliceStart { eval, frame, .. }
            | ContinuationSnapshot::SliceEnd { eval, frame, .. } => {
                self.checked_slice(eval)?;
                self.frame(frame, context)?;
                let bindings = frame.generic_bindings();
                match state {
                    ContinuationSnapshot::SliceStart { base, .. } => {
                        self.typed_expression_value(base, eval.base, &bindings)
                    }
                    ContinuationSnapshot::SliceEnd { base, start, .. } => {
                        self.typed_expression_value(base, eval.base, &bindings)?;
                        self.typed_expression_value(start, eval.start, &bindings)
                    }
                    _ => Ok(()),
                }
            }
            ContinuationSnapshot::PipelineInput { expr, span, frame }
            | ContinuationSnapshot::PipelineTarget {
                expr, span, frame, ..
            } => {
                let Some(HirExpr::Pipeline { input, .. }) = self.checked.hir.exprs.get(*expr)
                else {
                    return Err("checkpoint pipeline invocation has no checked owner".into());
                };
                crate::eval::pipeline::checked_pipeline_stages(self.checked, *expr, *span)?;
                self.frame(frame, context)?;
                if let ContinuationSnapshot::PipelineTarget { input: value, .. } = state {
                    let bindings = frame.generic_bindings();
                    self.typed_expression_value(value, *input, &bindings)?;
                }
                Ok(())
            }
            ContinuationSnapshot::PipelineStageTarget {
                expr,
                next_stage_index,
                targets,
                span,
                frame,
            } => {
                let stages =
                    crate::eval::pipeline::checked_pipeline_stages(self.checked, *expr, *span)?;
                self.index_at_most(*next_stage_index, stages.len(), context)?;
                if next_stage_index.checked_sub(1) != Some(targets.len()) {
                    return Err(
                        "checkpoint pipeline has an inconsistent resolved target prefix".into(),
                    );
                }
                self.frame(frame, context)?;
                let bindings = frame.generic_bindings();
                for (target, stage) in targets.iter().zip(stages) {
                    self.call_target_in(target, context, call_target::ReferenceScope::CheckedSlot)?;
                    let ty = *self
                        .checked
                        .types
                        .expr_types
                        .get(&stage.expr)
                        .ok_or("checkpoint pipeline stage has no checked type")?;
                    self.reference_target_type_with_bindings(target, ty, &bindings)?;
                }
                Ok(())
            }
            _ => Err(format!(
                "{context}: expected an access or pipeline continuation"
            )),
        }
    }

    fn checked_index(
        &self,
        expr: HirExprId,
        span: etas_core::Span,
    ) -> Result<(HirExprId, HirExprId), String> {
        match self.checked.hir.exprs.get(expr) {
            Some(HirExpr::Index {
                base,
                index,
                span: expected,
            }) if *expected == span && self.checked.types.index_facts.contains_key(&expr) => {
                Ok((*base, *index))
            }
            _ => Err("checkpoint index continuation has no matching checked owner".into()),
        }
    }

    fn checked_slice(&self, eval: &SliceExprEvalSnapshot) -> Result<(), String> {
        match self.checked.hir.exprs.get(eval.expr) {
            Some(HirExpr::Slice {
                base,
                start,
                end,
                bounds,
                span,
            }) if *base == eval.base
                && *start == eval.start
                && *end == eval.end
                && *bounds == eval.bounds
                && *span == eval.span
                && self.checked.types.slice_facts.contains_key(&eval.expr) =>
            {
                Ok(())
            }
            _ => Err("checkpoint slice descriptor differs from its checked owner".into()),
        }
    }
}
