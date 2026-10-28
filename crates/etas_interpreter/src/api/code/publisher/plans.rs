use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use etas_frontend::CheckedProgram;
use etas_hir::HirItemId;

use super::{CandidatePublicationError, InterpreterPlan};

/// Entry-specific execution facts share the immutable layouts and intrinsic ABI
/// of one checked program. No Host availability or authority is retained here.
pub(super) struct RevisionEntryPlans {
    program: Arc<crate::plan::PreparedProgram>,
    entries: BTreeMap<HirItemId, OnceLock<Result<Arc<InterpreterPlan>, String>>>,
}

impl RevisionEntryPlans {
    pub(super) fn build(
        program: &CheckedProgram,
        default: Arc<InterpreterPlan>,
        entries: impl Iterator<Item = HirItemId>,
    ) -> Result<Self, CandidatePublicationError> {
        let mut plans = BTreeMap::from([(default.entry.item, OnceLock::from(Ok(default.clone())))]);
        for entry in entries {
            if let std::collections::btree_map::Entry::Vacant(slot) = plans.entry(entry) {
                let plan = default
                    .select_entry(program, entry)
                    .map_err(|diagnostics| CandidatePublicationError::PlanRejected {
                        diagnostics,
                    })?;
                slot.insert(OnceLock::from(Ok(Arc::new(plan))));
            }
        }
        Ok(Self {
            program: default.prepared_program().clone(),
            entries: plans,
        })
    }

    pub(super) fn prepare_source(
        program: &CheckedProgram,
        entries: impl Iterator<Item = HirItemId>,
    ) -> Result<Self, CandidatePublicationError> {
        let program = crate::plan::prepare_program(program)
            .map_err(|diagnostics| CandidatePublicationError::PlanRejected { diagnostics })?;
        Ok(Self {
            program,
            entries: entries.map(|item| (item, OnceLock::new())).collect(),
        })
    }

    pub(super) fn program(&self) -> &Arc<crate::plan::PreparedProgram> {
        &self.program
    }

    pub(super) fn get(&self, item: HirItemId) -> Result<&Arc<InterpreterPlan>, String> {
        let entry = self
            .entries
            .get(&item)
            .ok_or("selected revision callable has no declared checked entry")?;
        entry.get_or_init(|| {
            self.program.select_callable(etas_frontend::CheckedCallableRoot::Item(item))
                .map(Arc::new)
                .map_err(|diagnostics| format!("selected revision callable has no checked execution plan: {diagnostics:?}"))
        }).as_ref().map_err(Clone::clone)
    }
}
