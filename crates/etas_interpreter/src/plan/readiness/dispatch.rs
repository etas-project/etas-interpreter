use etas_frontend::{CheckedCallableRoot, CheckedProject};
use etas_hir::{
    ExprView, HirExpr, HirVisitor, ResolveResult, SymbolDef, walk_expr_tree, walk_item,
};

use super::HostServiceCheck;

pub(super) fn service_check(
    project: &CheckedProject,
    root: CheckedCallableRoot,
) -> Result<HostServiceCheck, String> {
    let reachable = project
        .checked_callable_reachability(root)
        .map_err(|error| error.to_string())?;
    let view = project.hir.view();
    let mut visitor = Dispatches {
        project,
        late_bound: false,
        error: None,
    };
    if let CheckedCallableRoot::Lambda(expr) | CheckedCallableRoot::StdReference(expr) = root {
        walk_expr_tree(&view, expr, &mut visitor);
    }
    for item in reachable {
        walk_item(&view, item, &mut visitor);
    }
    if let Some(error) = visitor.error {
        return Err(error);
    }
    Ok(if visitor.late_bound {
        HostServiceCheck::EscapedBoundary
    } else {
        HostServiceCheck::Preflight
    })
}

struct Dispatches<'a> {
    project: &'a CheckedProject,
    late_bound: bool,
    error: Option<String>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for Dispatches<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if self
            .project
            .types
            .agent_method_calls
            .contains_key(&expr.id())
        {
            self.late_bound = true;
            return;
        }
        let HirExpr::Call { callee, .. } = expr.data() else {
            return;
        };
        if self.project.types.std_method_calls.contains_key(&expr.id()) {
            return;
        }
        // An Agent value pins a revision. A flow value can likewise carry a
        // native program and install its own handlers before invoking callbacks.
        if self
            .project
            .types
            .expr_types
            .get(callee)
            .is_some_and(|ty| self.project.types.agent_components.contains_key(ty))
        {
            self.late_bound = true;
            return;
        }
        let Some(HirExpr::Path(path)) = self.project.hir.exprs.get(*callee) else {
            self.late_bound = true;
            return;
        };
        let symbol = match &path.resolution {
            ResolveResult::Resolved(symbol) => *symbol,
            // Qualified constructors/members keep a checked namespace prefix.
            // This classifies dispatch ownership; it does not resolve the member.
            ResolveResult::PartiallyResolved(partial) => match partial.resolved_prefix {
                Some(symbol) => symbol,
                None => {
                    self.error = Some("readiness call has no resolved namespace owner".into());
                    return;
                }
            },
            _ => {
                self.error = Some("readiness call has no resolved callee identity".into());
                return;
            }
        };
        let Some(symbol) = self.project.symbols.get(symbol) else {
            self.error = Some("readiness call references a missing callee symbol".into());
            return;
        };
        if symbol.kind == etas_hir::SymbolKind::Agent {
            self.late_bound = true;
            return;
        }
        if !matches!(
            symbol.def,
            SymbolDef::Item { .. }
                | SymbolDef::ImportAlias { .. }
                | SymbolDef::EnumVariant { .. }
                | SymbolDef::ExternalEnumVariant { .. }
        ) {
            self.late_bound = true;
        }
    }
}
