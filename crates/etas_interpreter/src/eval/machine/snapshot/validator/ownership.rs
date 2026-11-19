use super::{LocalsSnapshot, frame::FrameValidation};
use crate::{
    control::BindingIdentity,
    driver::scheduler::{TaskId, TaskIdentity},
};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
pub(super) struct FrameOwnership {
    pub programs: HashMap<u64, usize>,
    pub logical_tasks: BTreeMap<TaskId, TaskIdentity>,
    pub deferred: bool,
    pub affinity_override: Option<TaskIdentity>,
    visits: HashSet<(u64, TaskIdentity)>,
    bindings: HashMap<u64, (BindingIdentity, usize, TaskIdentity)>,
    declaring_tasks: HashMap<u64, TaskIdentity>,
    declarations: HashMap<u64, (BindingIdentity, usize, TaskIdentity)>,
}

impl FrameOwnership {
    pub fn logical_task(&self, actual: Option<TaskIdentity>) -> TaskIdentity {
        match self
            .affinity_override
            .or(actual)
            .unwrap_or(TaskIdentity::Root)
        {
            TaskIdentity::Root => TaskIdentity::Root,
            TaskIdentity::Scheduled(id) => self
                .logical_tasks
                .get(&id)
                .copied()
                .unwrap_or(TaskIdentity::Scheduled(id)),
        }
    }

    pub fn observe(
        &mut self,
        frame: &LocalsSnapshot,
        program: usize,
        actual: Option<TaskIdentity>,
    ) -> Result<bool, String> {
        let logical = self.logical_task(actual);
        for binding in &frame.shared_bindings {
            if let Some((existing, owner_program, owner_task)) = self.bindings.get(&binding.id) {
                if existing != binding || *owner_program != program {
                    return Err("captured binding identity has conflicting declaring owners".into());
                }
                if *owner_task != logical {
                    return Err("captured mutable binding crosses logical tasks".into());
                }
            } else {
                self.bindings
                    .insert(binding.id, (*binding, program, logical));
            }
            if self
                .declaring_tasks
                .insert(binding.owner, logical)
                .is_some_and(|task| task != logical)
            {
                return Err("captured binding declaring activation crosses logical tasks".into());
            }
        }
        Ok(self.visits.insert((frame.id, logical)))
    }

    pub fn declare(
        &mut self,
        binding: BindingIdentity,
        program: usize,
        actual: Option<TaskIdentity>,
    ) -> Result<(), String> {
        let proof = (binding, program, self.logical_task(actual));
        if self
            .declarations
            .insert(binding.id, proof)
            .is_some_and(|previous| previous != proof)
        {
            return Err("captured binding has conflicting active declaration scopes".into());
        }
        Ok(())
    }

    pub fn validate(&self, definitions: &HashMap<u64, FrameValidation>) -> Result<(), String> {
        for state in definitions.values() {
            let FrameValidation::Complete(frame) = state else {
                continue;
            };
            for binding in &frame.shared_bindings {
                let Some(FrameValidation::Complete(owner)) = definitions.get(&binding.owner) else {
                    return Err("captured mutable binding has no live declaring activation".into());
                };
                if !owner.shared_bindings.contains(binding) {
                    return Err(
                        "captured mutable binding differs from its declaring activation".into(),
                    );
                }
                let value = frame
                    .locals
                    .iter()
                    .find(|(symbol, _)| *symbol == binding.symbol);
                let original = owner
                    .locals
                    .iter()
                    .find(|(symbol, _)| *symbol == binding.symbol);
                if value != original {
                    return Err(
                        "captured mutable binding aliases have different checkpoint values".into(),
                    );
                }
                let expected = self
                    .bindings
                    .get(&binding.id)
                    .ok_or("captured binding has no logical ownership proof")?;
                if self.declarations.get(&binding.id) != Some(expected) {
                    return Err("captured mutable binding has no active declaring ScopeExit".into());
                }
            }
        }
        Ok(())
    }
}
