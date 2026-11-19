use super::SnapshotValidator;
use crate::value::contract::{self, Context, Seed, Target, Value};
use etas_types::{ItemSignature, Type, TypeId};

impl SnapshotValidator<'_> {
    pub(super) fn program_boundary_site(
        &self,
        frame: &crate::orchestration::ProgramReturnSnapshot,
        context: &str,
    ) -> Result<(), String> {
        match &frame.call {
            crate::orchestration::ProgramCallSnapshot::Callable(_) => {
                self.invocation_site(&frame.call_site, frame.span, context)
            }
            crate::orchestration::ProgramCallSnapshot::Expression(expression) => {
                self.expression_unit(expression)?;
                self.expression_site(&frame.call_site, frame.span, context)?;
                self.expression_site(&expression.site, frame.span, context)
            }
        }
    }

    pub(crate) fn expression_unit(
        &self,
        call: &crate::orchestration::ExpressionCallSnapshot,
    ) -> Result<(), String> {
        if let crate::orchestration::ExpressionUnitSnapshot::Handler {
            caller,
            site,
            handler,
            ..
        } = &call.unit
        {
            let revisions = self
                .revisions
                .ok_or("Handler expression requires freshly checked owners")?;
            let caller = revisions.execution_snapshot(caller)?;
            let mut validator = SnapshotValidator::new(
                &caller.checked,
                &caller.plan.slots,
                &caller.plan.dispatch,
                &caller.plan.closures,
                self.limits,
            )
            .with_revisions(self.revisions);
            validator.frame_owners = self.frame_owners.clone();
            validator.frame_definitions = self.frame_definitions.clone();
            validator.tasks = self.tasks;
            validator.task_identity = self.task_identity;
            let crate::control::CallSiteKind::Expr { expr } = site.kind else {
                return Err("Handler expression has no caller expression".into());
            };
            let span = caller
                .checked
                .hir
                .exprs
                .get(expr)
                .ok_or("Handler caller expression is missing")?
                .span(&caller.checked.hir.blocks);
            validator.expression_site(site, span, "checkpoint Handler caller site")?;
            validator.validate_retained_value(handler)?;
        }
        Ok(())
    }

    pub(crate) fn scoped_handler(
        &self,
        metadata: &crate::value::ScopedHandlerMetadata,
        value: &crate::orchestration::ValueSnapshot,
    ) -> Result<
        (
            crate::value::CheckedExecutionProgram,
            Vec<crate::plan::code::RevisionProjection>,
            TypeId,
        ),
        String,
    > {
        let revisions = self
            .revisions
            .ok_or("scoped Handler requires freshly checked revisions")?;
        let (native, projections) = metadata.resolve(revisions)?;
        let mut program = &native.checked;
        let mut ty = metadata.native_type;
        for edge in &projections {
            if !std::sync::Arc::ptr_eq(program, edge.source()) {
                return Err("scoped Handler projection has a different source owner".into());
            }
            ty = edge.runtime_type(ty)?;
            program = edge.target();
        }
        if !std::sync::Arc::ptr_eq(program, self.checked) {
            return Err("scoped Handler has a different receiving program".into());
        }
        let mut validator = SnapshotValidator::new(
            &native.checked,
            &native.plan.slots,
            &native.plan.dispatch,
            &native.plan.closures,
            self.limits,
        )
        .with_revisions(self.revisions);
        validator.frame_owners = self.frame_owners.clone();
        validator.frame_definitions = self.frame_definitions.clone();
        validator.tasks = self.tasks;
        validator.task_identity = self.task_identity;
        validator.validate_retained_value(value)?;
        validator.reference_value(
            value,
            metadata.native_type,
            &crate::control::GenericBindings::new(),
        )?;
        Ok((native, projections, ty))
    }
    pub(crate) fn validate_retained_value(
        &self,
        value: &crate::orchestration::ValueSnapshot,
    ) -> Result<(), String> {
        self.snapshot_value_in(value, super::call_target::ReferenceScope::CheckedSlot)
    }

    pub(crate) fn validate_policy_value(
        &self,
        entry: etas_hir::HirItemId,
        value: &crate::orchestration::ValueSnapshot,
    ) -> Result<(), String> {
        self.snapshot_value_in(value, super::call_target::ReferenceScope::CheckedSlot)?;
        let Some(ItemSignature::Flow(signature)) = self.checked.types.item_signatures.get(&entry)
        else {
            return Err("source policy entry has no checked signature".into());
        };
        let Some(ty) = signature.params.first() else {
            return Err("source policy entry has no policy parameter".into());
        };
        self.reference_value(value, *ty, &crate::control::GenericBindings::new())
    }

    pub(super) fn handler_capture(
        &self,
        expr: etas_hir::HirExprId,
        captured: Option<&crate::orchestration::LocalsSnapshot>,
    ) -> Result<(), String> {
        let layout = self
            .closures
            .get(expr)
            .ok_or("handler has no checked capture layout")?;
        let Some(captured) = captured else {
            return if layout.captures.is_empty() {
                Ok(())
            } else {
                Err("handler is missing its captured environment".into())
            };
        };
        let original = captured;
        let resolved = self.resolve_frame(captured)?;
        let captured = &resolved;
        let symbols = captured
            .locals
            .iter()
            .map(|(symbol, _)| *symbol)
            .collect::<std::collections::BTreeSet<_>>();
        if symbols.iter().copied().ne(layout.captures.iter().copied()) || captured.parent.is_some()
        {
            return Err("handler environment differs from its checked capture layout".into());
        }
        if captured
            .shared_bindings
            .iter()
            .map(|binding| binding.symbol)
            .ne(layout.mutable_captures.iter().copied())
        {
            return Err(
                "handler mutable captures differ from their checked binding identities".into(),
            );
        }
        self.frame(original, "checkpoint handler capture")
    }
    pub(crate) fn validate_task_start(
        &self,
        call: &crate::orchestration::CallTargetSnapshot,
    ) -> Result<TypeId, String> {
        self.call_target(call, "task start origin")?;
        let crate::orchestration::CallTargetSnapshot::StdIntrinsic {
            origin,
            parameter_types,
            result_type,
            ..
        } = call
        else {
            let (_, _, result) = crate::eval::task_start::source_start_types(self.checked, call)?;
            return Ok(result);
        };
        if !self.checked.concurrency.sites.contains_key(origin)
            || etas_types::source_execution::task(
                &self.checked.type_store,
                &self.checked.types.known_std_types,
                *result_type,
            )
            .is_none()
            || !matches!(parameter_types.as_slice(), [ty] if matches!(self.checked.type_store.get(*ty), Some(Type::Function(flow)) if flow.input.is_empty()))
        {
            return Err("task start origin differs from its checked scheduler call".into());
        }
        Ok(*result_type)
    }

    pub(crate) fn validate_task_outcome(
        &self,
        outcome: &crate::orchestration::TaskOutcomeSnapshot,
        task_type: TypeId,
    ) -> Result<(), String> {
        match outcome {
            crate::orchestration::TaskOutcomeSnapshot::Completed(value) => {
                self.validate_retained_value(value)?;
                let Some(task) = etas_types::source_execution::task(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types,
                    task_type,
                ) else {
                    return Err("task outcome has no checked result signature".into());
                };
                self.reference_value(value, task.output, &crate::control::GenericBindings::new())
            }
            crate::orchestration::TaskOutcomeSnapshot::Raised {
                error_type,
                payload,
                ..
            } => {
                self.validate_retained_value(payload)?;
                self.reference_value(
                    payload,
                    *error_type,
                    &crate::control::GenericBindings::new(),
                )
            }
            _ => Ok(()),
        }
    }

    pub(super) fn validate_argument_contract(
        &self,
        item: etas_hir::HirItemId,
        values: &[crate::orchestration::ValueSnapshot],
    ) -> Result<(), String> {
        let signature = match self.checked.types.item_signatures.get(&item) {
            Some(ItemSignature::Flow(signature) | ItemSignature::Tool(signature)) => {
                Some(signature)
            }
            Some(ItemSignature::Agent(agent)) => agent.callable(),
            _ => None,
        }
        .ok_or("checkpoint entry has no checked callable signature")?;
        if signature.params.len() != values.len() {
            return Err("checkpoint arguments do not match checked entry arity".into());
        }
        contract::validate(
            self,
            values
                .iter()
                .zip(&signature.params)
                .map(|(value, ty)| Seed {
                    value: Value::Snapshot(value),
                    ty: *ty,
                }),
            &crate::control::GenericBindings::new(),
        )
    }

    pub(crate) fn handler_value_contract(
        &self,
        expr: etas_hir::HirExprId,
        arms: &[crate::orchestration::ActiveHandlerArmRecord],
        expected: TypeId,
    ) -> Result<(), String> {
        if self.checked.types.expr_types.get(&expr) != Some(&expected)
            || !matches!(
                self.checked.type_store.get(expected),
                Some(Type::Handler(_))
            )
        {
            return Err("handler input has a different checked type".into());
        }
        let Some(etas_hir::HirExpr::Handler { handlers, .. }) = self.checked.hir.exprs.get(expr)
        else {
            return Err("handler input has no checked literal identity".into());
        };
        if arms.iter().map(|arm| arm.arm).ne(handlers.iter().copied()) {
            return Err("handler input has a different checked arm layout".into());
        }
        if !self.checked.effects.handler_values.contains_key(&expr) {
            return Err("handler input has no checked effect facts".into());
        }
        for arm in arms {
            self.handler_arm(arm, "handler input")?;
        }
        Ok(())
    }
}

impl Context for SnapshotValidator<'_> {
    fn existential_header(
        &self,
        header: &crate::value::ExistentialHeader,
    ) -> Result<crate::control::GenericBindings, String> {
        header.validate_snapshot(self.checked, self.revisions)
    }
    fn checked(&self) -> &etas_frontend::CheckedProject {
        self.checked
    }
    fn limits(&self) -> &etas_host::StorageLimits {
        self.limits
    }
    fn callable(&self, target: Target<'_>, expected: TypeId) -> Result<(), String> {
        let Target::Snapshot(target) = target else {
            return Err("snapshot input contains a runtime callable".into());
        };
        self.reference_target_type(target, expected)
    }
    fn comparator(&self, target: Target<'_>, key: TypeId) -> Result<(), String> {
        let Target::Snapshot(target) = target else {
            return Err("snapshot comparator is not a snapshot callable".into());
        };
        self.key_comparator_interface(target, key, &crate::control::GenericBindings::new())
    }
    fn native(&self, value: Value<'_>, expected: TypeId) -> Result<(), String> {
        match value {
            Value::Snapshot(crate::orchestration::ValueSnapshot::ScopedHandler {
                metadata,
                value,
            }) => {
                let (_, _, ty) = self.scoped_handler(metadata, value)?;
                if ty != expected {
                    return Err("scoped Handler has a different checked slot type".into());
                }
                Ok(())
            }
            Value::Snapshot(crate::orchestration::ValueSnapshot::TaskRuntime(value)) => {
                use crate::orchestration::TaskValueSnapshot as T;
                let scheduler = self
                    .tasks
                    .ok_or("task value requires its scheduler snapshot")?;
                scheduler.validate_value(*value)?;
                let known = &self.checked.types.known_std_types.execution;
                match (value, self.checked.type_store.get(expected)) {
                    (T::Task { .. }, _) if known.task_handle == Some(expected) => Ok(()),
                    (T::Scheduler { .. }, _) if known.scheduler_state == Some(expected) => Ok(()),
                    (T::Continuation { .. }, _) if known.continuation_state == Some(expected) => {
                        Ok(())
                    }
                    (T::Task { task, .. }, Some(Type::Task(ty)))
                        if self.task_output_type(crate::driver::scheduler::TaskId(*task))?
                            == ty.output =>
                    {
                        Ok(())
                    }
                    (T::Scheduler { .. }, Some(Type::SchedulerHandle)) => Ok(()),
                    (T::Continuation { continuation, .. }, Some(Type::Continuation(ty)))
                        if scheduler.continuations.entries
                            [&crate::driver::scheduler::ContinuationId(*continuation)]
                            .output
                            == ty.action_output
                            || scheduler.continuations.entries
                                [&crate::driver::scheduler::ContinuationId(*continuation)]
                                .binder
                                .and_then(|symbol| {
                                    self.checked.types.continuation_bindings.get(&symbol)
                                })
                                .is_some_and(|binding| {
                                    binding.action_output == ty.action_output
                                }) =>
                    {
                        Ok(())
                    }
                    _ => Err(format!(
                        "task snapshot value {value:?} differs from checked slot type `{}` ({:?})",
                        etas_types::ty::display_type(&self.checked.type_store, expected),
                        self.checked.type_store.get(expected)
                    )),
                }
            }
            Value::Snapshot(crate::orchestration::ValueSnapshot::CodeIdentity { ty, identity })
                if *ty == expected =>
            {
                identity
                    .restore(&self.checked.types.known_std_types.code, *ty)
                    .map(|_| ())
            }
            Value::Snapshot(crate::orchestration::ValueSnapshot::Handler {
                fact_expr,
                handlers,
                ..
            }) => self.handler_value_contract(*fact_expr, handlers, expected),
            _ => Err("snapshot native input has a different checked type identity".into()),
        }
    }
}
