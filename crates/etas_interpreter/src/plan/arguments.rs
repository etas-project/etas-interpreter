use std::{collections::HashMap, sync::Arc};

use etas_hir::{HirArg, HirExpr, HirExprId};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use super::context::{PlanContext, plan_pass};

/// Immutable HIR argument descriptors shared by repeated calls and suspension.
/// Evaluated arguments remain invocation-local and preserve source order.
#[derive(Clone, Debug, Default)]
pub(crate) struct CallArgumentTable {
    entries: HashMap<HirExprId, Arc<[HirArg]>>,
}

impl CallArgumentTable {
    fn build(hir: &etas_hir::HirProgram) -> Self {
        let entries = hir
            .exprs
            .iter()
            .filter_map(|(expr, data)| match data {
                HirExpr::Call { args, .. } | HirExpr::Perform { args, .. } => {
                    Some((expr, Arc::from(args.as_slice())))
                }
                _ => None,
            })
            .collect();
        Self { entries }
    }

    pub(crate) fn get(&self, expr: HirExprId) -> Option<Arc<[HirArg]>> {
        self.entries.get(&expr).cloned()
    }
}

pub(super) struct BuildCallArgumentsPass;

impl Pass<PlanContext<'_>> for BuildCallArgumentsPass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_call_arguments")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _: &PassContext<PlanContext<'_>>,
        _: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        context.arguments = Some(CallArgumentTable::build(&context.project.hir));
        PassResult::unchanged()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{allocation::measure, project::checked_project};

    #[test]
    fn call_descriptor_ownership_is_per_site_and_acquisition_is_allocation_free() {
        let checked = checked_project(
            "module app.main; flow f(value: i32) -> i32 { return value; } flow main() -> i32 { return f(value = 1) + f(value = 2); }",
        );
        let (table, cold_cost) = measure(|| CallArgumentTable::build(&checked.hir));
        assert_eq!(table.entries.len(), 2);
        let sites = table.entries.keys().copied().collect::<Vec<_>>();
        let first = table.get(sites[0]).unwrap();
        let other = table.get(sites[1]).unwrap();
        assert!(!Arc::ptr_eq(&first, &other));
        let HirArg::Named {
            value: first_value, ..
        } = &first[0]
        else {
            panic!("named argument")
        };
        let HirArg::Named {
            value: other_value, ..
        } = &other[0]
        else {
            panic!("named argument")
        };
        assert_ne!(first_value, other_value);
        let (alias, cost) = measure(|| table.get(sites[0]).unwrap());
        assert!(Arc::ptr_eq(&first, &alias));
        assert_eq!(cost.count, 0);
        assert_eq!(cost.bytes, 0);
        assert!(table.get(HirExprId(u32::MAX)).is_none());
        eprintln!("two call descriptor sites cold={cold_cost:?}, lookup={cost:?}");
        // A retained continuation owns its one descriptor, not the entire table.
        let discarded = Arc::downgrade(&other);
        drop(other);
        drop(table);
        assert!(discarded.upgrade().is_none());
        assert!(Arc::ptr_eq(&first, &alias));
    }
}
