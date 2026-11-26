use etas_hir::{HirExpr, HirExprId};
use etas_types::TypeId;

use super::{ExpressionCall, ExpressionUnit};
use crate::{
    control::{
        CallSite, CallSiteKind, Continuation, ContinuationInput, ControlSignal, Frame,
        GenericBindings, PendingProgramExpr,
    },
    eval::EvalContext,
    plan::code::RevisionProjection,
    value::{CheckedExecutionProgram, InterpValue, ScopedHandlerValue},
};

#[derive(Clone, Debug)]
pub(crate) struct HandlerApplication {
    pub(crate) caller: CheckedExecutionProgram,
    pub(crate) site: CallSite,
    pub(crate) handler: ScopedHandlerValue,
    pub(crate) body: bool,
}

impl HandlerApplication {
    pub(crate) fn new(
        caller: CheckedExecutionProgram,
        site: CallSite,
        handler: ScopedHandlerValue,
    ) -> Result<ExpressionCall, String> {
        let application = Self {
            caller,
            site,
            handler,
            body: false,
        };
        application.expression()
    }

    pub(crate) fn expression(&self) -> Result<ExpressionCall, String> {
        let (body, handler_expr) = self.checked_site()?;
        let bindings = self.site.frame.bindings_at(
            &self.caller.checked,
            etas_hir::HirOwner::Expr(self.handle_expr()?),
        );
        let caller_type = self
            .caller
            .checked
            .types
            .expr_types
            .get(&handler_expr)
            .ok_or("Handler application has no checked Handler input type")?;
        let caller_type = bindings
            .substitution(&self.caller.checked.type_store)
            .substitute(*caller_type)
            .map_err(|error| error.to_string())?;
        if self.handler.presented(&self.caller.checked)? != caller_type {
            return Err("Handler application changes its checked Handler interface".into());
        }
        let source = if self.body {
            self.handler.program.clone()
        } else {
            self.caller.clone()
        };
        let destination = if self.body {
            self.caller.clone()
        } else {
            self.handler.program.clone()
        };
        let expr = if self.body { body } else { self.handle_expr()? };
        let mut output = bindings
            .substitution(&self.caller.checked.type_store)
            .substitute(
                *self
                    .caller
                    .checked
                    .types
                    .expr_types
                    .get(&expr)
                    .ok_or("handled expression has no checked output type")?,
            )
            .map_err(|error| error.to_string())?;
        if !self.body {
            for edge in self
                .handler
                .projections
                .iter()
                .rev()
                .map(RevisionProjection::reversed)
            {
                output = edge.runtime_type(output)?;
            }
        }
        let site = if self.body {
            let (fact_expr, _, captured) = self.handler.parts()?;
            let frame = captured
                .cloned()
                .unwrap_or_else(|| Frame::new(self.handler.program.plan.slots.clone()));
            CallSite {
                kind: CallSiteKind::Expr { expr: fact_expr },
                frame,
                receiver_update: None,
                composition: None,
                task_callback: None,
            }
        } else {
            self.site.clone()
        };
        Ok(ExpressionCall {
            tool: None,
            source,
            destination,
            site,
            slots: Vec::new(),
            bindings: GenericBindings::new(),
            output,
            projection: None,
            unit: ExpressionUnit::Handler(Box::new(self.clone())),
        })
    }

    pub(crate) fn handle_expr(&self) -> Result<HirExprId, String> {
        match self.site.kind {
            CallSiteKind::Expr { expr } => Ok(expr),
            _ => Err("Handler application has no checked expression origin".into()),
        }
    }

    pub(crate) fn checked_site(&self) -> Result<(HirExprId, HirExprId), String> {
        let expr = self.handle_expr()?;
        let Some(HirExpr::Handle { body, handler, .. }) = self.caller.checked.hir.exprs.get(expr)
        else {
            return Err("Handler application does not name an actual handle expression".into());
        };
        let fact = self
            .caller
            .checked
            .effects
            .handle_applications
            .get(&expr)
            .ok_or("Handler application has no checked effect contract")?;
        if fact.handler != (etas_effects::HandlerValueRef::Expr { expr: *handler }) {
            return Err("Handler application changes its checked Handler origin".into());
        }
        Ok((*body, *handler))
    }

    pub(crate) fn enter(
        &self,
        call: &ExpressionCall,
        ctx: &mut EvalContext<'_>,
        span: etas_core::Span,
    ) -> ControlSignal {
        let missing = |reason| ControlSignal::missing_checked_fact(reason, span);
        if self.body {
            let (body, _) = match self.checked_site() {
                Ok(site) => site,
                Err(reason) => return missing(reason),
            };
            return ctx.eval_expr(body, &mut self.site.frame.clone());
        }
        if let Err(reason) = self.handler.validate(
            &self.caller.checked,
            match self.handler.presented(&self.caller.checked) {
                Ok(ty) => ty,
                Err(reason) => return missing(reason),
            },
            &ctx.storage_limits,
        ) {
            return missing(reason);
        }
        let (fact_expr, handlers, captured) = match self.handler.parts() {
            Ok(parts) => parts,
            Err(reason) => return missing(reason),
        };
        if captured.is_some_and(crate::value::graph::has_expired_captures) {
            return ControlSignal::invalid_arguments(
                "Handler retaining a local var cannot run after its owning scope ends",
                span,
            );
        }
        let mut body = self.clone();
        body.body = true;
        let body = match body.expression() {
            Ok(body) => body,
            Err(reason) => return missing(reason),
        };
        let native_span = ctx.checked.hir.exprs[fact_expr].span(&ctx.checked.hir.blocks);
        let initial = ControlSignal::ProgramExpr(Box::new(PendingProgramExpr {
            call: body,
            args: Vec::new(),
            span: native_span,
            continuation: Continuation::BlockValue,
        }));
        let mut frame = captured
            .cloned()
            .unwrap_or_else(|| Frame::new(ctx.plan.slots.clone()));
        let _ = call;
        ctx.eval_handle_with_signal(initial, handlers.to_vec(), native_span, &mut frame)
    }

    pub(crate) fn action_projections(&self) -> Vec<RevisionProjection> {
        if self.body {
            self.handler
                .projections
                .iter()
                .rev()
                .map(RevisionProjection::reversed)
                .collect()
        } else {
            self.handler.projections.clone()
        }
    }

    pub(crate) fn control(
        &self,
        input: ContinuationInput,
        program: &CheckedExecutionProgram,
    ) -> Result<ContinuationInput, String> {
        let expr = self.handle_expr()?;
        let exits = self.caller.checked.lexical_exit_types(expr)?;
        let (ty, value, construct): (_, _, fn(InterpValue) -> ContinuationInput) = match input {
            ContinuationInput::Return(value) => (exits.returning, value, ContinuationInput::Return),
            ContinuationInput::Resume(value) => (exits.resuming, value, ContinuationInput::Resume),
            ContinuationInput::Finish(value) => (exits.finishing, value, ContinuationInput::Finish),
            input @ (ContinuationInput::Break | ContinuationInput::Continue) => return Ok(input),
            _ => return Err("Handler lexical exit contains a different continuation input".into()),
        };
        let ty = ty.ok_or("Handler expression has no checked lexical exit payload type")?;
        let mut ty = self
            .site
            .frame
            .bindings_at(&self.caller.checked, etas_hir::HirOwner::Expr(expr))
            .substitution(&self.caller.checked.type_store)
            .substitute(ty)
            .map_err(|error| error.to_string())?;
        if !self.body {
            for edge in self
                .handler
                .projections
                .iter()
                .rev()
                .map(RevisionProjection::reversed)
            {
                ty = edge.runtime_type(ty)?;
            }
        }
        let (_, value) = self.project_outgoing(ty, value, program)?;
        Ok(construct(value))
    }

    pub(crate) fn project_outgoing(
        &self,
        mut ty: TypeId,
        mut value: InterpValue,
        program: &CheckedExecutionProgram,
    ) -> Result<(TypeId, InterpValue), String> {
        for edge in self.action_projections() {
            (ty, value) = edge.value(ty, value, program)?;
        }
        Ok((ty, value))
    }

    pub(crate) fn result(
        &self,
        ty: TypeId,
        value: InterpValue,
        program: &CheckedExecutionProgram,
    ) -> Result<InterpValue, String> {
        self.project_outgoing(ty, value, program)
            .map(|(_, value)| value)
    }
}
