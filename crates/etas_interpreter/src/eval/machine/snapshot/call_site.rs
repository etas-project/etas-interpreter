use super::{CaptureContext, RestoreContext};
use crate::{
    control::{CallSite, CallSiteKind},
    orchestration::CallSiteSnapshot,
};

impl CallSiteSnapshot {
    pub(in crate::eval::machine) fn capture(
        site: &CallSite,
        context: &mut CaptureContext,
    ) -> Result<Self, String> {
        Ok(Self {
            kind: site.kind,
            frame: context.frame(&site.frame)?,
            composition: site
                .composition
                .as_ref()
                .map(|composition| {
                    Ok::<_, String>(crate::orchestration::ComposedInvocationSnapshot {
                        root: context.call_target(&composition.root)?.into(),
                        path: composition.path.indices(),
                    })
                })
                .transpose()?,
        })
    }

    pub(in crate::eval::machine) fn restore(
        self,
        context: &mut RestoreContext,
    ) -> Result<CallSite, String> {
        Ok(CallSite {
            kind: self.kind,
            frame: context.frame(self.frame)?,
            composition: self
                .composition
                .map(|composition| {
                    Ok::<_, String>(crate::control::ComposedInvocation {
                        root: super::call_target::restore_call_target(
                            composition.root.into_value(),
                            context,
                        )?
                        .into(),
                        path: crate::control::InvocationPath::from_indices(&composition.path),
                    })
                })
                .transpose()?,
        })
    }

    pub(in crate::eval::machine) fn receiver(
        &self,
        parent: &etas_frontend::CheckedProgram,
        span: etas_core::Span,
    ) -> Result<(etas_hir::HirExprId, Option<etas_hir::HirExprId>), String> {
        use etas_hir::HirExpr;
        match self.kind {
            CallSiteKind::Pipeline { .. } => {
                Err("program boundary has an unselected pipeline invocation".into())
            }
            CallSiteKind::Expr { expr } => match parent.hir.exprs.get(expr) {
                Some(HirExpr::Call {
                    callee,
                    span: expected,
                    ..
                }) if *expected == span => Ok((*callee, Some(expr))),
                Some(HirExpr::MethodCall {
                    receiver,
                    span: expected,
                    ..
                }) if *expected == span => Ok((*receiver, Some(expr))),
                _ => {
                    Err("program boundary references a missing or changed checked call site".into())
                }
            },
            CallSiteKind::PipelineStage { expr, index } => {
                let Some(HirExpr::Pipeline { .. }) = parent.hir.exprs.get(expr) else {
                    return Err("program boundary pipeline invocation has no checked owner".into());
                };
                let stages = crate::eval::pipeline::checked_pipeline_stages(parent, expr, span)?;
                let stage = stages
                    .get(index)
                    .ok_or("program boundary pipeline stage is out of bounds")?;
                Ok((stage.expr, None))
            }
        }
    }
}

impl crate::orchestration::ComposedInvocationSnapshot {
    pub(crate) fn stages_and_index(
        &self,
    ) -> Result<(&[crate::orchestration::CallTargetSnapshot], usize), String> {
        use crate::orchestration::CallTargetSnapshot as Target;
        let (last, parents) = self
            .path
            .split_last()
            .ok_or("checkpoint composition has no stage path")?;
        let mut target = &*self.root;
        for (index, final_step) in parents
            .iter()
            .map(|index| (*index, false))
            .chain(std::iter::once((*last, true)))
        {
            while let Target::Limited { target: inner, .. }
            | Target::Specialized { target: inner, .. } = target
            {
                target = inner;
            }
            let Target::Composed(stages) = target else {
                return Err("checkpoint invocation path does not select a composition".into());
            };
            if final_step {
                if index > stages.len() {
                    return Err("checkpoint composition stage is out of bounds".into());
                }
                return Ok((stages, index));
            }
            target = stages
                .get(index)
                .ok_or("checkpoint composition stage is out of bounds")?;
        }
        Err("checkpoint composition has no stage path".into())
    }

    pub(crate) fn selected(&self) -> Result<&crate::orchestration::CallTargetSnapshot, String> {
        use crate::orchestration::CallTargetSnapshot as Target;
        let (stages, index) = self.stages_and_index()?;
        let mut target = stages
            .get(index)
            .ok_or("checkpoint composition stage is out of bounds")?;
        while let Target::Limited { target: inner, .. }
        | Target::Specialized { target: inner, .. } = target
        {
            target = inner;
        }
        Ok(target)
    }
}
