use std::collections::{HashMap, HashSet};

use etas_builtin::PureIntrinsicRegistry;
use etas_frontend::CheckedProject;
use etas_hir::{
    ExprView, HirExpr, HirExprId, HirTreeView, HirVisitor, ResolveResult, SymbolDef, SymbolId,
    walk_item,
};
use etas_std::{IntrinsicDispatch, StdIntrinsicId};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use crate::intrinsic::{
    dispatch::{StdCallable, StdIntrinsicIdentity},
    pure::PureAbiProjector,
    registry::StdIntrinsicHandlerRegistry,
};

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct IntrinsicDispatchTable {
    pub pure_registry: PureIntrinsicRegistry,
    pure_abi: PureAbiProjector,
    handlers: StdIntrinsicHandlerRegistry,
    std_intrinsics: HashMap<SymbolId, StdIntrinsicIdentity>,
    imported_intrinsics: HashMap<StdIntrinsicId, IntrinsicDispatch>,
    reachable_intrinsics: HashSet<StdIntrinsicId>,
    map_exprs: HashSet<HirExprId>,
    brace_literals: HashMap<HirExprId, BraceLiteralShape>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BraceLiteralShape {
    Record,
    Map,
}

impl IntrinsicDispatchTable {
    pub(crate) fn for_project(project: &CheckedProject) -> Result<Self, Vec<String>> {
        let handlers = StdIntrinsicHandlerRegistry::build(&project.std_registry);
        let mut std_intrinsics = HashMap::new();
        let mut imported_intrinsics = HashMap::new();
        let mut errors = Vec::new();
        for symbol in project.symbols.iter() {
            let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                continue;
            };
            let Some(descriptor) = project
                .std_registry
                .lookup_qualified(path)
                .and_then(|std_symbol| std_symbol.intrinsic.as_ref())
            else {
                continue;
            };
            if let Err(error) = handlers.validate_descriptor(descriptor) {
                errors.push(error);
                continue;
            }
            if let Some(existing) = imported_intrinsics.insert(descriptor.id, descriptor.dispatch)
                && existing != descriptor.dispatch
            {
                errors.push(format!(
                    "standard intrinsic {} is imported with conflicting {:?} and {:?} dispatch categories",
                    descriptor.id.0, existing, descriptor.dispatch
                ));
                continue;
            }
            std_intrinsics.insert(
                symbol.id,
                StdIntrinsicIdentity {
                    intrinsic: descriptor.id,
                    dispatch: descriptor.dispatch,
                },
            );
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let reachable_intrinsics =
            collect_reachable_intrinsics(project, &std_intrinsics).map_err(|error| vec![error])?;

        let mut map_exprs = HashSet::new();
        let mut brace_literals = HashMap::new();
        for (expr, ty) in project.types.expr_types.iter() {
            match project.type_store.get(*ty) {
                Some(etas_types::Type::Record(_)) => {
                    brace_literals.insert(*expr, BraceLiteralShape::Record);
                }
                Some(etas_types::Type::Map { .. }) => {
                    map_exprs.insert(*expr);
                    brace_literals.insert(*expr, BraceLiteralShape::Map);
                }
                _ => {}
            }
        }

        Ok(Self {
            pure_registry: PureIntrinsicRegistry,
            pure_abi: PureAbiProjector::build(&project.type_store),
            handlers,
            std_intrinsics,
            imported_intrinsics,
            reachable_intrinsics,
            map_exprs,
            brace_literals,
        })
    }

    pub fn std_intrinsic(&self, symbol: SymbolId) -> Option<StdIntrinsicIdentity> {
        self.std_intrinsics.get(&symbol).copied()
    }

    pub fn resolve_std_callable(
        &self,
        identity: StdIntrinsicIdentity,
    ) -> Result<StdCallable, String> {
        self.validate_std_intrinsic(identity)?;
        self.handlers
            .executable(identity.intrinsic, identity.dispatch)
    }

    pub fn validate_std_intrinsic(&self, identity: StdIntrinsicIdentity) -> Result<(), String> {
        let Some(imported_dispatch) = self.imported_intrinsics.get(&identity.intrinsic).copied()
        else {
            return Err(format!(
                "standard intrinsic {} is not imported by the current checked interpreter plan",
                identity.intrinsic.0
            ));
        };
        if imported_dispatch != identity.dispatch {
            return Err(format!(
                "standard intrinsic {} dispatch mismatch: current plan uses {:?}, checkpoint/call target declares {:?}",
                identity.intrinsic.0, imported_dispatch, identity.dispatch
            ));
        }
        if !self.reachable_intrinsics.contains(&identity.intrinsic) {
            return Err(format!(
                "standard intrinsic {} is not reachable from the current checked interpreter entry",
                identity.intrinsic.0
            ));
        }
        if !self
            .handlers
            .contains(identity.intrinsic, identity.dispatch)
        {
            return Err(format!(
                "standard intrinsic {} has no {:?} handler in the current interpreter plan",
                identity.intrinsic.0, identity.dispatch
            ));
        }
        Ok(())
    }

    pub fn validate_pure_intrinsic(&self, intrinsic: StdIntrinsicId) -> Result<(), String> {
        self.validate_std_intrinsic(StdIntrinsicIdentity {
            intrinsic,
            dispatch: IntrinsicDispatch::PureKernel,
        })
    }

    pub fn pure_abi(&self) -> &PureAbiProjector {
        &self.pure_abi
    }

    pub fn is_map_expr(&self, expr: HirExprId) -> bool {
        self.map_exprs.contains(&expr)
    }

    pub fn brace_literal_shape(&self, expr: HirExprId) -> Option<BraceLiteralShape> {
        self.brace_literals.get(&expr).copied()
    }
}

fn collect_reachable_intrinsics(
    project: &CheckedProject,
    std_intrinsics: &HashMap<SymbolId, StdIntrinsicIdentity>,
) -> Result<HashSet<StdIntrinsicId>, String> {
    let view = HirTreeView::try_new(&project.hir)
        .map_err(|error| format!("cannot index checked HIR for intrinsic reachability: {error}"))?;
    let mut visitor = ReachableIntrinsicVisitor {
        std_intrinsics,
        reachable: HashSet::new(),
    };
    for item in &project.reachability.reachable_items {
        if view.item(*item).is_none() {
            return Err(format!(
                "entry reachability references missing HIR item {}",
                item.0
            ));
        }
        walk_item(&view, *item, &mut visitor);
    }
    Ok(visitor.reachable)
}

struct ReachableIntrinsicVisitor<'a> {
    std_intrinsics: &'a HashMap<SymbolId, StdIntrinsicIdentity>,
    reachable: HashSet<StdIntrinsicId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for ReachableIntrinsicVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        let HirExpr::Path(path) = expr.data() else {
            return;
        };
        let ResolveResult::Resolved(symbol) = path.resolution else {
            return;
        };
        if let Some(identity) = self.std_intrinsics.get(&symbol) {
            self.reachable.insert(identity.intrinsic);
        }
    }
}

pub(super) struct BuildIntrinsicDispatchTablePass;

impl Pass<PlanContext<'_>> for BuildIntrinsicDispatchTablePass {
    fn descriptor(&self) -> etas_utils::PassDescriptor {
        plan_pass("interpreter.plan.build_intrinsic_dispatch")
    }

    fn run(
        &mut self,
        context: &mut PlanContext<'_>,
        _pass_context: &PassContext<PlanContext<'_>>,
        _manager: &mut PassManager<PlanContext<'_>>,
    ) -> PassResult {
        match IntrinsicDispatchTable::for_project(context.project) {
            Ok(dispatch) => context.dispatch = Some(dispatch),
            Err(errors) => {
                let span = crate::diagnostics::primary_span(context.project);
                for error in errors {
                    context.push_missing_fact(span, &error);
                }
            }
        }
        PassResult::unchanged()
    }
}

#[cfg(test)]
mod tests {
    use super::IntrinsicDispatchTable;

    #[test]
    fn imported_runtime_intrinsic_without_handler_fails_closed() {
        let checked = crate::testing::project::checked_project(
            r#"
module app.main;

import std.runtime.time.now;

flow main() -> unit {
  return;
}
"#,
        );
        let errors = IntrinsicDispatchTable::for_project(&checked)
            .expect_err("imported runtime intrinsic without a handler must fail closed");
        assert!(
            errors.iter().any(|error| {
                error.contains("std.runtime.time.now")
                    && error.contains("no registered Runtime handler")
            }),
            "{errors:?}"
        );
    }

    #[test]
    fn imported_but_entry_unreachable_intrinsic_is_not_checkpoint_executable() {
        let checked = crate::testing::project::checked_project(
            r#"
module app.main;

import std.io.println;

flow unused() -> unit ![Console, Error<IOError>] {
  println("unreachable");
}

flow main() -> unit {
  return;
}
"#,
        );
        let dispatch = IntrinsicDispatchTable::for_project(&checked).expect("plan should build");
        let identity = checked
            .symbols
            .iter()
            .find_map(|symbol| dispatch.std_intrinsic(symbol.id))
            .expect("println import should have an intrinsic identity");
        let error = dispatch
            .validate_std_intrinsic(identity)
            .expect_err("entry-unreachable intrinsic must not be checkpoint executable");
        assert!(error.contains("not reachable"), "{error}");
    }
}
