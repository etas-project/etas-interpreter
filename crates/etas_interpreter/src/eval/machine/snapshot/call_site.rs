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
            task_callback: site
                .task_callback
                .as_ref()
                .map(|call| {
                    context
                        .call_target(&crate::control::CallTarget::StdIntrinsic((**call).clone()))
                        .map(Box::new)
                })
                .transpose()?,
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
            task_callback: self
                .task_callback
                .map(|call| match context.restore_call_target(*call)? {
                    crate::control::CallTarget::StdIntrinsic(call) => Ok(Box::new(call)),
                    _ => Err("task callback has no checked primitive origin".to_owned()),
                })
                .transpose()?,
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

    pub(in crate::eval::machine) fn bindings(
        &self,
        checked: &etas_frontend::CheckedProgram,
    ) -> crate::control::GenericBindings {
        let (CallSiteKind::Expr { expr }
        | CallSiteKind::TaskCallback { expr }
        | CallSiteKind::Pipeline { expr }
        | CallSiteKind::PipelineStage { expr, .. }) = self.kind;
        self.frame
            .bindings_at(checked, etas_hir::HirOwner::Expr(expr))
    }

    pub(in crate::eval::machine) fn receiver(
        &self,
        parent: &etas_frontend::CheckedProgram,
        span: etas_core::Span,
    ) -> Result<(etas_hir::HirExprId, Option<etas_hir::HirExprId>), String> {
        use etas_hir::HirExpr;
        if let CallSiteKind::TaskCallback { expr } = self.kind {
            self.task_callback_type(parent)?;
            return Ok((expr, None));
        }
        if self.task_callback.is_some() {
            return Err("ordinary call carries a task callback origin".into());
        }
        match self.kind {
            CallSiteKind::TaskCallback { .. } => unreachable!(),
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
    pub(in crate::eval::machine) fn task_callback_type(
        &self,
        parent: &etas_frontend::CheckedProgram,
    ) -> Result<etas_types::TypeId, String> {
        let CallSiteKind::TaskCallback { expr } = self.kind else {
            return Err("call site is not a task callback".into());
        };
        if self.composition.is_some() {
            return Err("task callback cannot be a pipeline stage".into());
        }
        let Some(crate::orchestration::CallTargetSnapshot::StdIntrinsic {
            origin,
            substitutions,
            intrinsic,
            dispatch,
            parameter_types,
            result_type,
        }) = self.task_callback.as_deref()
        else {
            return Err("task callback has no checked primitive origin".into());
        };
        if *origin != expr {
            return Err("task callback differs from its primitive origin".into());
        }
        crate::plan::IntrinsicDispatchTable::recheck_task_callback(
            parent,
            &crate::intrinsic::dispatch::CheckedStdIntrinsicCall {
                origin: *origin,
                substitutions: substitutions.clone(),
                identity: crate::intrinsic::dispatch::StdIntrinsicIdentity {
                    intrinsic: *intrinsic,
                    dispatch: *dispatch,
                },
                parameter_types: parameter_types.clone(),
                result_type: *result_type,
            },
        )
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
