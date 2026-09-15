use super::{BTreeSet, CallTargetSnapshot, SnapshotValidator, TypeId};
use std::collections::HashSet;

enum Pending<'a> {
    Bindings(&'a [(String, TypeId)]),
    Siblings(&'a [CallTargetSnapshot]),
}

impl SnapshotValidator<'_> {
    pub(super) fn call_target(
        &self,
        root: &CallTargetSnapshot,
        context: &str,
    ) -> Result<(), String> {
        let mut next = Some(root);
        let mut pending = Vec::new();
        // Snapshot edges are immutable and acyclic. Repeated aliases need the
        // same checks once, while each enclosing wrapper keeps its own bindings.
        let mut nodes = HashSet::new();
        let mut tables = HashSet::new();
        loop {
            if let Some(node) = next.take() {
                match node {
                    CallTargetSnapshot::Specialized {
                        target,
                        type_bindings,
                    } => {
                        // Bindings are checked after the target, before its next sibling.
                        if !type_bindings.is_empty() {
                            pending.push(Pending::Bindings(type_bindings));
                        }
                        if target.shared_identity().is_none_or(|key| nodes.insert(key)) {
                            next = Some(target);
                        }
                    }
                    CallTargetSnapshot::Limited { target, .. } => {
                        if target.shared_identity().is_none_or(|key| nodes.insert(key)) {
                            next = Some(target);
                        }
                    }
                    CallTargetSnapshot::Composed(targets) => {
                        if targets
                            .shared_identity()
                            .is_some_and(|key| !tables.insert(key))
                        {
                            continue;
                        }
                        if let Some((first, remaining)) = targets.split_first() {
                            if !remaining.is_empty() {
                                pending.push(Pending::Siblings(remaining));
                            }
                            next = Some(first);
                        }
                    }
                    leaf => self.call_target_leaf(leaf, context)?,
                }
                continue;
            }
            match pending.pop() {
                Some(Pending::Bindings(bindings)) => {
                    let mut names = BTreeSet::new();
                    for (name, ty) in bindings {
                        if name.is_empty() || !names.insert(name) {
                            return Err(format!(
                                "{context}: specialized call target contains an invalid or duplicate type parameter"
                            ));
                        }
                        self.type_id(*ty, context)?;
                    }
                }
                Some(Pending::Siblings(siblings)) => {
                    if let Some((first, remaining)) = siblings.split_first() {
                        if !remaining.is_empty() {
                            pending.push(Pending::Siblings(remaining));
                        }
                        next = Some(first);
                    }
                }
                None => return Ok(()),
            }
        }
    }

    fn call_target_leaf(&self, target: &CallTargetSnapshot, context: &str) -> Result<(), String> {
        match target {
            CallTargetSnapshot::FlowItem(item)
            | CallTargetSnapshot::AgentItem(item)
            | CallTargetSnapshot::ToolItem(item) => self.item(*item, context),
            CallTargetSnapshot::SpecImplMethod(symbol) => self.symbol(*symbol, context),
            CallTargetSnapshot::EnumVariant(symbol) => {
                self.symbol(*symbol, context)?;
                if self.dispatch.enum_constructor(*symbol).is_some()
                    || self.checked.symbols.get(*symbol).is_some_and(|symbol| {
                        matches!(symbol.def, etas_hir::SymbolDef::EnumVariant { .. })
                    })
                {
                    Ok(())
                } else {
                    Err(format!(
                        "{context}: call target is not a checked enum constructor"
                    ))
                }
            }
            CallTargetSnapshot::NominalConstructor(ty) => self.type_id(*ty, context),
            CallTargetSnapshot::PureIntrinsic {
                intrinsic,
                parameter_types,
                result_type,
            } => {
                self.dispatch
                    .validate_pure_intrinsic(*intrinsic)
                    .map_err(|error| format!("{context}: {error}"))?;
                for parameter_type in parameter_types {
                    self.type_id(*parameter_type, context)?;
                }
                self.type_id(*result_type, context)
            }
            CallTargetSnapshot::Lambda { expr, captured } => {
                self.expr(*expr, context)?;
                let layout = self
                    .closures
                    .get(*expr)
                    .ok_or_else(|| format!("{context}: lambda has no checked capture layout"))?;
                let symbols = captured
                    .locals
                    .iter()
                    .map(|(symbol, _)| *symbol)
                    .collect::<BTreeSet<_>>();
                if !layout
                    .captures
                    .iter()
                    .all(|symbol| symbols.contains(symbol))
                {
                    return Err(format!("{context}: lambda is missing a required capture"));
                }
                if symbols
                    .iter()
                    .any(|symbol| layout.slots.resolve(*symbol).is_none())
                {
                    return Err(format!(
                        "{context}: lambda contains a binding outside its checked layout"
                    ));
                }
                self.frame(captured, context)
            }
            CallTargetSnapshot::StdIntrinsic {
                intrinsic,
                dispatch,
                parameter_types,
                result_type,
            } => {
                self.dispatch
                    .resolve_std_callable(crate::intrinsic::dispatch::StdIntrinsicIdentity {
                        intrinsic: *intrinsic,
                        dispatch: *dispatch,
                    })
                    .map_err(|error| format!("{context}: {error}"))?;
                for parameter_type in parameter_types {
                    self.type_id(*parameter_type, context)?;
                }
                self.type_id(*result_type, context)
            }
            CallTargetSnapshot::Specialized { .. }
            | CallTargetSnapshot::Limited { .. }
            | CallTargetSnapshot::Composed(_) => Err(format!(
                "{context}: internal call target traversal sent a parent to leaf validation"
            )),
        }
    }
}
