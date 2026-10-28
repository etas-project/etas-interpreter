use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use etas_builtin::PureIntrinsicRegistry;
use etas_frontend::{CheckedCallableRoot, CheckedProject};
use etas_hir::{
    ExprView, HirExpr, HirExprId, HirTreeView, HirVisitor, ResolveResult, SymbolDef, SymbolId,
    walk_expr_tree, walk_item,
};
use etas_std::{IntrinsicDispatch, StdIntrinsicId};
use etas_utils::{Pass, PassContext, PassManager, PassResult};

use crate::intrinsic::{
    dispatch::{CheckedStdIntrinsicCall, StdCallable, StdIntrinsicIdentity},
    pure::PureAbiProjector,
    registry::StdIntrinsicHandlerRegistry,
};
mod checked_calls;
mod checked_methods;
mod default_actions;
mod forwarded_actions;

use super::context::{PlanContext, plan_pass};

#[derive(Clone, Debug, Default)]
pub struct IntrinsicDispatchTable {
    program: Arc<ProgramDispatch>,
    reflection_required: bool,
    reachable_intrinsics: HashSet<StdIntrinsicId>,
    pub pure_registry: PureIntrinsicRegistry,
}

#[derive(Debug, Default)]
struct ProgramDispatch {
    compiler_actions: HashMap<etas_effects::ActionRef, forwarded_actions::CheckedCompilerAction>,
    forwarded_actions: HashMap<StdIntrinsicId, forwarded_actions::CheckedForwardedAction>,
    default_actions: HashMap<HirExprId, default_actions::CheckedDefaultAction>,
    default_action_intrinsics: HashSet<StdIntrinsicId>,
    std_methods: HashMap<HirExprId, CheckedStdIntrinsicCall>,
    std_type_members: HashSet<HirExprId>,
    std_call_origins: HashMap<HirExprId, CheckedStdIntrinsicCall>,
    pure_abi: PureAbiProjector,
    handlers: StdIntrinsicHandlerRegistry,
    std_intrinsics: HashMap<SymbolId, StdIntrinsicIdentity>,
    enum_constructors: HashMap<SymbolId, StdEnumConstructor>,
    imported_intrinsics: HashMap<StdIntrinsicId, IntrinsicDispatch>,
    callable_roots: HashMap<IntrinsicCallableAbi, HirExprId>,
    std_callable_origins: HashSet<HirExprId>,
    map_exprs: HashSet<HirExprId>,
    brace_literals: HashMap<HirExprId, BraceLiteralShape>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
struct IntrinsicCallableAbi {
    intrinsic: u32,
    input: Vec<etas_types::TypeId>,
    output: etas_types::TypeId,
}

#[derive(Clone, Debug)]
pub(crate) struct StdEnumConstructor {
    pub name: String,
    pub arity: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BraceLiteralShape {
    Record,
    Map,
}

impl IntrinsicDispatchTable {
    pub(crate) fn for_project(project: &CheckedProject) -> Result<Self, Vec<String>> {
        let root = project
            .entry
            .map(CheckedCallableRoot::Item)
            .ok_or_else(|| {
                vec!["intrinsic dispatch has no selected checked execution entry".into()]
            })?;
        Self::for_program(project)?
            .select_callable(project, root)
            .map_err(|error| vec![error])
    }

    fn for_program(project: &CheckedProject) -> Result<Self, Vec<String>> {
        let handlers = StdIntrinsicHandlerRegistry::build(&project.std_registry);
        let mut std_intrinsics = HashMap::new();
        let mut enum_constructors = HashMap::new();
        let mut imported_intrinsics = HashMap::new();
        let mut errors = Vec::new();
        for symbol in project.symbols.iter() {
            let SymbolDef::ImportAlias { path, .. } = &symbol.def else {
                continue;
            };
            if let Some(std_symbol) = project.std_registry.lookup_qualified(path)
                && std_symbol.enum_owner.is_some()
            {
                let etas_std::StdDecl::Flow(declaration) = &std_symbol.decl else {
                    errors.push(format!(
                        "standard enum constructor {} has no signature",
                        path.join(".")
                    ));
                    continue;
                };
                if !matches!(
                    project.types.symbol_types.get(&symbol.id),
                    Some(etas_types::SymbolTypeFact::Flow { .. })
                ) {
                    errors.push(format!(
                        "standard enum constructor {} has no checked signature",
                        path.join(".")
                    ));
                    continue;
                }
                enum_constructors.insert(
                    symbol.id,
                    StdEnumConstructor {
                        name: std_symbol.name.clone(),
                        arity: declaration.params.len(),
                    },
                );
                continue;
            }
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
        let (std_methods, std_type_members) =
            checked_methods::lower(project, &handlers, &mut imported_intrinsics)
                .map_err(|error| vec![error])?;
        let std_call_origins = checked_calls::lower(project, &std_intrinsics, &std_methods)
            .map_err(|error| vec![error])?;
        let forwarded_actions =
            forwarded_actions::lower(project, &imported_intrinsics).map_err(|error| vec![error])?;
        let compiler_actions =
            forwarded_actions::lower_compilers(project).map_err(|error| vec![error])?;
        let (default_actions, default_action_intrinsics) =
            default_actions::lower(project, &imported_intrinsics, &std_call_origins)
                .map_err(|error| vec![error])?;
        let mut callable_roots = HashMap::new();
        let mut std_callable_origins = HashSet::new();
        for (expr, fact) in &project.effects.std_callable_values {
            project
                .checked_callable_effects(CheckedCallableRoot::StdReference(*expr))
                .map_err(|error| vec![error.to_string()])?;
            let Some(etas_types::Type::Function(signature)) =
                project.type_store.get(fact.flow_type)
            else {
                return Err(vec![
                    "standard callable root has no checked function type".into(),
                ]);
            };
            std_callable_origins.insert(*expr);
            let key = IntrinsicCallableAbi {
                intrinsic: fact.intrinsic,
                input: signature.input.clone(),
                output: signature.output,
            };
            let previous = callable_roots.entry(key).or_insert(*expr);
            *previous = (*previous).min(*expr);
        }
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

        let pure_abi = PureAbiProjector::build(&project.type_store, &project.types.enum_layouts)
            .map_err(|error| {
                vec![format!(
                    "checked pure intrinsic ABI projection failed: {error}"
                )]
            })?;

        Ok(Self {
            reflection_required: false,
            reachable_intrinsics: HashSet::new(),
            pure_registry: PureIntrinsicRegistry,
            program: Arc::new(ProgramDispatch {
                compiler_actions,
                forwarded_actions,
                default_actions,
                default_action_intrinsics,
                std_methods,
                std_type_members,
                std_call_origins,
                pure_abi,
                handlers,
                std_intrinsics,
                enum_constructors,
                imported_intrinsics,
                callable_roots,
                std_callable_origins,
                map_exprs,
                brace_literals,
            }),
        })
    }

    pub(super) fn select_callable(
        &self,
        project: &CheckedProject,
        root: CheckedCallableRoot,
    ) -> Result<Self, String> {
        let (reachable_intrinsics, reflection_required) = collect_reachable_intrinsics(
            project,
            Some(root),
            &self.program.std_intrinsics,
            &self.program.std_methods,
        )?;
        Ok(Self {
            program: self.program.clone(),
            reflection_required,
            reachable_intrinsics,
            pure_registry: PureIntrinsicRegistry,
        })
    }

    pub fn std_intrinsic(&self, symbol: SymbolId) -> Option<StdIntrinsicIdentity> {
        self.program.std_intrinsics.get(&symbol).copied()
    }

    pub(crate) fn intrinsic_callable_root(
        &self,
        identity: StdIntrinsicIdentity,
        input: &[etas_types::TypeId],
        output: etas_types::TypeId,
    ) -> Result<CheckedCallableRoot, String> {
        self.validate_std_intrinsic(identity)?;
        self.intrinsic_reference_root(identity.intrinsic, input, output)
    }

    /// Resolves a checked reference identity, not permission to execute it.
    pub(crate) fn intrinsic_reference_root(
        &self,
        intrinsic: StdIntrinsicId,
        input: &[etas_types::TypeId],
        output: etas_types::TypeId,
    ) -> Result<CheckedCallableRoot, String> {
        self.program
            .callable_roots
            .get(&IntrinsicCallableAbi {
                intrinsic: intrinsic.0,
                input: input.to_vec(),
                output,
            })
            .copied()
            .map(CheckedCallableRoot::StdReference)
            .ok_or_else(|| {
                "standard callable has no checked reference and effect summary for its ABI".into()
            })
    }

    pub(crate) fn std_callable_root(
        &self,
        call: &CheckedStdIntrinsicCall,
    ) -> Result<CheckedCallableRoot, String> {
        self.validate_std_call_origin(call.origin, call.identity)?;
        if !self.program.std_callable_origins.contains(&call.origin) {
            return Err("standard callable origin has no checked reference effect summary".into());
        }
        Ok(CheckedCallableRoot::StdReference(call.origin))
    }

    pub(crate) fn std_method(&self, expr: HirExprId) -> Option<&CheckedStdIntrinsicCall> {
        self.program.std_methods.get(&expr)
    }

    pub(crate) fn std_type_member(&self, expr: HirExprId) -> Option<&CheckedStdIntrinsicCall> {
        self.program
            .std_type_members
            .contains(&expr)
            .then(|| self.program.std_methods.get(&expr))
            .flatten()
    }

    pub(crate) fn reflection_required(&self) -> bool {
        self.reflection_required
    }

    pub(crate) fn enum_constructor(&self, symbol: SymbolId) -> Option<&StdEnumConstructor> {
        self.program.enum_constructors.get(&symbol)
    }

    pub fn resolve_std_callable(
        &self,
        identity: StdIntrinsicIdentity,
    ) -> Result<StdCallable, String> {
        self.validate_std_intrinsic(identity)?;
        self.program
            .handlers
            .executable(identity.intrinsic, identity.dispatch)
    }

    pub fn validate_std_intrinsic(&self, identity: StdIntrinsicIdentity) -> Result<(), String> {
        let Some(imported_dispatch) = self
            .program
            .imported_intrinsics
            .get(&identity.intrinsic)
            .copied()
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
            .program
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
        &self.program.pure_abi
    }

    pub fn is_map_expr(&self, expr: HirExprId) -> bool {
        self.program.map_exprs.contains(&expr)
    }

    pub fn brace_literal_shape(&self, expr: HirExprId) -> Option<BraceLiteralShape> {
        self.program.brace_literals.get(&expr).copied()
    }
}

fn collect_reachable_intrinsics(
    project: &CheckedProject,
    root: Option<CheckedCallableRoot>,
    std_intrinsics: &HashMap<SymbolId, StdIntrinsicIdentity>,
    std_methods: &HashMap<HirExprId, CheckedStdIntrinsicCall>,
) -> Result<(HashSet<StdIntrinsicId>, bool), String> {
    let view = HirTreeView::try_new(&project.hir)
        .map_err(|error| format!("cannot index checked HIR for intrinsic reachability: {error}"))?;
    let mut visitor = ReachableIntrinsicVisitor {
        std_intrinsics,
        std_methods,
        reachable: HashSet::new(),
        reflection_required: false,
    };
    let root = root.ok_or("intrinsic dispatch has no selected checked execution entry")?;
    let reachable = project
        .checked_callable_reachability(root)
        .map_err(|error| error.to_string())?;
    if let CheckedCallableRoot::Lambda(expr) | CheckedCallableRoot::StdReference(expr) = root {
        walk_expr_tree(&view, expr, &mut visitor);
    }
    for item in &reachable {
        if view.item(*item).is_none() {
            return Err(format!(
                "entry reachability references missing HIR item {}",
                item.0
            ));
        }
        walk_item(&view, *item, &mut visitor);
    }
    Ok((visitor.reachable, visitor.reflection_required))
}

struct ReachableIntrinsicVisitor<'a> {
    reflection_required: bool,
    std_methods: &'a HashMap<HirExprId, CheckedStdIntrinsicCall>,
    std_intrinsics: &'a HashMap<SymbolId, StdIntrinsicIdentity>,
    reachable: HashSet<StdIntrinsicId>,
}

impl<'view, 'hir> HirVisitor<'view, 'hir> for ReachableIntrinsicVisitor<'_> {
    fn enter_expr(&mut self, expr: ExprView<'view, 'hir>) {
        if matches!(expr.data(), HirExpr::Reflect(_)) {
            self.reflection_required = true;
        }
        if let Some(call) = self.std_methods.get(&expr.id()) {
            self.reachable.insert(call.identity.intrinsic);
        }
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
        match IntrinsicDispatchTable::for_program(context.project) {
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
