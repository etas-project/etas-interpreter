use super::SnapshotValidator;
use crate::value::contract::{self, Context, Seed, Target, Value};
use etas_types::{ItemSignature, Type, TypeId};

impl SnapshotValidator<'_> {
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
