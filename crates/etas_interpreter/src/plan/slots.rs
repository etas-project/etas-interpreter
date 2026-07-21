use std::collections::HashMap;

use etas_core::id_type;
use etas_frontend::CheckedProject;
use etas_hir::{SymbolDef, SymbolId, SymbolKind};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};

id_type!(SlotId);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotLayoutTable {
    symbols: Vec<SymbolId>,
    by_symbol: HashMap<SymbolId, SlotId>,
}

impl SlotLayoutTable {
    pub fn for_project(project: &CheckedProject) -> Self {
        let mut symbols = project
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(symbol.kind, SymbolKind::Param | SymbolKind::Local)
                    || matches!(symbol.def, SymbolDef::PatternBinding { .. })
            })
            .map(|symbol| symbol.id)
            .collect::<Vec<_>>();
        symbols.sort_by_key(|symbol| symbol.0);
        symbols.dedup();

        let by_symbol = symbols
            .iter()
            .enumerate()
            .map(|(index, symbol)| (*symbol, SlotId(index as u32)))
            .collect();

        Self { symbols, by_symbol }
    }

    pub fn resolve(&self, symbol: SymbolId) -> Option<SlotId> {
        self.by_symbol.get(&symbol).copied()
    }

    pub fn slot_count(&self) -> usize {
        self.symbols.len()
    }

    pub fn symbol_for_slot(&self, slot: SlotId) -> Option<SymbolId> {
        self.symbols.get(slot.0 as usize).copied()
    }

    pub fn symbols(&self) -> &[SymbolId] {
        &self.symbols
    }
}

pub(super) struct BuildSlotLayoutPass;

impl Pass<PlanContext<'_>> for BuildSlotLayoutPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_slot_layout")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.slots = Some(SlotLayoutTable::for_project(context.project));
        PassResult::unchanged()
    }
}
