use super::SnapshotValidator;
use crate::value::contract::{self, Context, Seed, Target, Value};
use etas_types::{ItemSignature, Type, TypeId};

impl SnapshotValidator<'_> {
    pub(crate) fn validate_task_start(
        &self,
        call: &crate::orchestration::CallTargetSnapshot,
    ) -> Result<(), String> {
        self.call_target(call, "task start origin")?;
        let crate::orchestration::CallTargetSnapshot::StdIntrinsic {
            origin,
            parameter_types,
            result_type,
            ..
        } = call
        else {
            return Err("task start origin has no standard call ABI".into());
        };
        if !self.checked.concurrency.sites.contains_key(origin)
            || !matches!(
                self.checked.type_store.get(*result_type),
                Some(Type::Task(_))
            )
            || !matches!(parameter_types.as_slice(), [ty] if matches!(self.checked.type_store.get(*ty), Some(Type::Function(flow)) if flow.input.is_empty()))
        {
            return Err("task start origin differs from its checked scheduler call".into());
        }
        Ok(())
    }

    pub(crate) fn validate_task_outcome(
        &self,
        outcome: &crate::orchestration::TaskOutcomeSnapshot,
        task_type: TypeId,
    ) -> Result<(), String> {
        match outcome {
            crate::orchestration::TaskOutcomeSnapshot::Completed(value) => {
                let Some(Type::Task(task)) = self.checked.type_store.get(task_type) else {
                    return Err("task outcome has no checked result signature".into());
                };
                self.reference_value(value, task.output, &crate::control::GenericBindings::new())
            }
            crate::orchestration::TaskOutcomeSnapshot::Raised {
                error_type,
                payload,
                ..
            } => self.reference_value(
                payload,
                *error_type,
                &crate::control::GenericBindings::new(),
            ),
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
            Value::Snapshot(crate::orchestration::ValueSnapshot::TaskRuntime(value)) => {
                use crate::orchestration::TaskValueSnapshot as T;
                let scheduler = self
                    .tasks
                    .ok_or("task value requires its scheduler snapshot")?;
                scheduler.validate_value(*value)?;
                match (value, self.checked.type_store.get(expected)) {
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
            }) => self.handler_value_contract(*fact_expr, handlers, expected),
            _ => Err("snapshot native input has a different checked type identity".into()),
        }
    }
}
