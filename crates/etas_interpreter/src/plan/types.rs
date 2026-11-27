use crate::control::GenericBindings;
use etas_types::TypeId;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use etas_core::Diagnostic;

use crate::api::EntryPoint;

use super::{
    ActionMediationTable, GlobalTable, HostRequirementTable, IntrinsicDispatchTable, ResourceTable,
    SlotLayoutTable,
};

#[derive(Clone, Debug)]
pub struct InterpreterPlan {
    pub(crate) standard_library: Option<Arc<super::standard_library::StandardLibraryProgram>>,
    pub(super) prepared: Arc<PreparedProgram>,
    pub(super) program: etas_frontend::CheckedProject,
    pub root: etas_frontend::CheckedCallableRoot,
    pub(crate) known_std_types: crate::eval::KnownStdTypes,
    pub(super) spec_facts: Arc<std::sync::OnceLock<etas_types::SpecFacts>>,
    pub entry: EntryPoint,
    pub slots: Arc<SlotLayoutTable>,
    pub(crate) frames: Arc<super::frames::FrameLayoutTable>,
    pub(crate) arguments: Arc<super::arguments::CallArgumentTable>,
    pub closures: Arc<super::ClosureLayoutTable>,
    pub named_variants: Arc<super::NamedVariantLayoutTable>,
    pub records: Arc<super::RecordLayoutTable>,
    pub globals: Arc<GlobalTable>,
    pub resources: Arc<ResourceTable>,
    pub dispatch: IntrinsicDispatchTable,
    pub host_requirements: HostRequirementTable,
    pub action_mediation: ActionMediationTable,
    pub(crate) residual: Arc<super::residual::PreparedResidualPlans>,
    pub(crate) residual_installation: super::residual::ResidualInstallationEvidence,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub(super) struct CallablePlan {
    pub entry: EntryPoint,
    pub dispatch: IntrinsicDispatchTable,
    pub host_requirements: HostRequirementTable,
    pub action_mediation: ActionMediationTable,
    pub installation: super::residual::ResidualInstallationEvidence,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ExpressionRoute {
    Literal,
    Operand,
    Local(etas_hir::SymbolId),
    ToolReference,
}

/// Checked program-wide data has no executable root or invocation authority.
#[derive(Debug)]
pub(crate) struct PreparedProgram {
    pub(super) native_applications: Mutex<HashMap<etas_hir::HirItemId, GenericBindings>>,
    pub(super) callable_plans: Mutex<
        HashMap<
            (
                std::mem::Discriminant<etas_frontend::CheckedCallableRoot>,
                u32,
            ),
            CallablePlan,
        >,
    >,
    // Each checked program owns its type IDs. Keep only the last application per
    // source type, including the full type and effect environment in the key.
    pub(super) materializations: Mutex<HashMap<TypeId, (GenericBindings, TypeId)>>,
    pub(super) enum_layouts: Mutex<super::code::CheckedValueTypes>,
    pub(super) sequence_layouts: Mutex<HashSet<TypeId>>,
    pub(crate) expression_routes: HashMap<etas_hir::HirExprId, ExpressionRoute>,
    pub(crate) numeric_literals: super::arithmetic::NumericLiterals,
    pub(crate) arithmetic:
        std::collections::HashMap<etas_hir::HirItemId, super::arithmetic::ArithmeticBody>,
    pub(super) standard_library: Option<Arc<super::standard_library::StandardLibraryProgram>>,
    pub(super) program: etas_frontend::CheckedProject,
    pub(super) known_std_types: crate::eval::KnownStdTypes,
    pub(super) spec_facts: Arc<std::sync::OnceLock<etas_types::SpecFacts>>,
    pub(super) slots: Arc<SlotLayoutTable>,
    pub(super) frames: Arc<super::frames::FrameLayoutTable>,
    pub(super) arguments: Arc<super::arguments::CallArgumentTable>,
    pub(super) closures: Arc<super::ClosureLayoutTable>,
    pub(super) named_variants: Arc<super::NamedVariantLayoutTable>,
    pub(super) records: Arc<super::RecordLayoutTable>,
    pub(super) globals: Arc<GlobalTable>,
    pub(super) resources: Arc<ResourceTable>,
    pub(super) dispatch: IntrinsicDispatchTable,
    pub(super) residual: Arc<super::residual::PreparedResidualPlans>,
    pub(super) diagnostics: Vec<Diagnostic>,
}

impl PreparedProgram {
    pub(crate) fn verify_native_application(
        &self,
        item: etas_hir::HirItemId,
        bindings: &GenericBindings,
    ) -> Result<(), String> {
        if self.native_applications.lock().unwrap().get(&item) == Some(bindings) {
            return Ok(());
        }
        crate::value::ScopedCallableValue::checked_item_application(
            &self.program,
            etas_frontend::CheckedCallableRoot::Item(item),
            bindings,
        )?
        .ok_or("specialized method has no checked native application")?;
        self.native_applications
            .lock()
            .unwrap()
            .insert(item, bindings.clone());
        Ok(())
    }

    pub(crate) fn construct_enum(
        &self,
        ty: TypeId,
        name: &str,
        fields: Vec<crate::value::InterpValue>,
    ) -> Result<crate::value::InterpValue, String> {
        crate::value::enum_value::construct_with_layouts(
            ty,
            name,
            fields,
            &self.program,
            &mut self.enum_layouts.lock().unwrap(),
        )
    }

    pub(crate) fn enum_payload<'a>(
        &self,
        value: &'a crate::value::InterpValue,
        owner: TypeId,
    ) -> Result<(&'a str, &'a [crate::value::InterpValue]), String> {
        crate::value::enum_value::payload_with_layouts(
            value,
            owner,
            &self.program,
            &mut self.enum_layouts.lock().unwrap(),
        )
    }

    pub(crate) fn materialized_type(
        &self,
        ty: TypeId,
        bindings: &GenericBindings,
    ) -> Result<TypeId, etas_types::TypeSubstitutionError> {
        if let Some((previous, result)) = self.materializations.lock().unwrap().get(&ty)
            && previous == bindings
        {
            return Ok(*result);
        }
        let result = bindings
            .indexed_substitution(
                &self.program.type_store,
                self.dispatch.type_lookup(&self.program),
            )
            .substitute(ty)?;
        self.materializations
            .lock()
            .unwrap()
            .insert(ty, (bindings.clone(), result));
        Ok(result)
    }

    pub(crate) fn has_sequence_layout(
        &self,
        owner: TypeId,
    ) -> Result<bool, etas_types::TypeSubstitutionError> {
        if self.sequence_layouts.lock().unwrap().contains(&owner) {
            return Ok(true);
        }
        if etas_types::value_representation::sequence_element_in_store(
            &self.program.type_store,
            owner,
        )?
        .is_some()
        {
            self.sequence_layouts.lock().unwrap().insert(owner);
            return Ok(true);
        }
        Ok(false)
    }
}
