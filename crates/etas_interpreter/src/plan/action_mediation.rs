use std::collections::BTreeSet;

use etas_effects::{ActionRef, Effect, EffectSummary};
use etas_hir::HirItem;
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use crate::diagnostics::item_span;

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct ActionMediationTable {
    pub requested_actions: BTreeSet<ActionRef>,
    pub default_actions: BTreeSet<ActionRef>,
}

impl ActionMediationTable {
    pub fn requires_action(&self, action: &ActionRef) -> bool {
        self.requested_actions.contains(action) || self.default_actions.contains(action)
    }

    pub fn has_default_action(&self, action: &ActionRef) -> bool {
        self.default_actions.contains(action)
    }

    fn include_summary(&mut self, summary: &EffectSummary) {
        self.requested_actions
            .extend(action_refs(&summary.requested_actions));
        self.default_actions
            .extend(action_refs(&summary.default_actions));
    }
}

pub(super) struct ComputeEntryActionMediationPass;

impl Pass<PlanContext<'_>> for ComputeEntryActionMediationPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.compute_entry_action_mediation")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        let Some(entry) = context.entry.map(|entry| entry.item) else {
            return PassResult::unchanged();
        };
        let Some(summary) = context.project.effects.item_effects.get(&entry) else {
            context.push_missing_fact(
                item_span(context.project, entry),
                "checked project is missing effect summary for action mediation",
            );
            return PassResult::unchanged();
        };
        let mut table = ActionMediationTable::default();
        table.include_summary(summary);
        for (item, hir_item) in context.project.hir.items.iter() {
            if !matches!(hir_item, HirItem::TopLevelLet(_)) {
                continue;
            }
            if let Some(summary) = context.project.effects.item_effects.get(&item) {
                table.include_summary(summary);
            }
        }
        context.action_mediation = Some(table);
        PassResult::unchanged()
    }
}

fn action_refs(row: &etas_effects::EffectRow) -> BTreeSet<ActionRef> {
    row.effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Action(action) => Some(action.clone()),
            Effect::AppliedAction(action) => Some(action.action.clone()),
            _ => None,
        })
        .collect()
}
