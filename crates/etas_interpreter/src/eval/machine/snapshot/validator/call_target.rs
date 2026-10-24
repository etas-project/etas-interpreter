use super::{BTreeSet, CallTargetSnapshot, SnapshotValidator, TypeId};
use crate::orchestration::{CallTargetSnapshotChildren, CallTargetSnapshotLink};
use std::collections::HashMap;

// Own immutable source edges so a completed check cannot be confused with a
// later allocation at the same address. COW edits acquire a different identity.
#[derive(Default)]
pub(super) struct ValidatedCallTargets {
    nodes: HashMap<*const CallTargetSnapshot, CallTargetSnapshotLink>,
    tables: HashMap<*const Vec<CallTargetSnapshot>, CallTargetSnapshotChildren>,
}

#[cfg(test)]
thread_local! {
    pub(super) static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

enum Pending<'a> {
    Bindings(&'a [(String, TypeId)]),
    Siblings(&'a [CallTargetSnapshot]),
    CompleteNode(*const CallTargetSnapshot, &'a CallTargetSnapshotLink),
    CompleteTable(
        *const Vec<CallTargetSnapshot>,
        &'a CallTargetSnapshotChildren,
    ),
}

impl SnapshotValidator<'_> {
    pub(super) fn call_target(
        &self,
        root: &CallTargetSnapshot,
        context: &str,
    ) -> Result<(), String> {
        let mut next = Some(root);
        let mut pending = Vec::new();
        loop {
            if let Some(node) = next.take() {
                #[cfg(test)]
                VISITS.set(VISITS.get() + 1);
                match node {
                    CallTargetSnapshot::Specialized {
                        target,
                        type_bindings,
                    } => {
                        // Bindings are checked after the target, before its next sibling.
                        if !type_bindings.is_empty() {
                            pending.push(Pending::Bindings(type_bindings));
                        }
                        if let Some(key) = target.shared_identity() {
                            if !self
                                .validated_call_targets
                                .borrow()
                                .nodes
                                .contains_key(&key)
                            {
                                pending.push(Pending::CompleteNode(key, target));
                                next = Some(target);
                            }
                        } else {
                            next = Some(target);
                        }
                    }
                    CallTargetSnapshot::Limited { target, .. } => {
                        if let Some(key) = target.shared_identity() {
                            if !self
                                .validated_call_targets
                                .borrow()
                                .nodes
                                .contains_key(&key)
                            {
                                pending.push(Pending::CompleteNode(key, target));
                                next = Some(target);
                            }
                        } else {
                            next = Some(target);
                        }
                    }
                    CallTargetSnapshot::Composed(targets) => {
                        if let Some(key) = targets.shared_identity() {
                            if self
                                .validated_call_targets
                                .borrow()
                                .tables
                                .contains_key(&key)
                            {
                                continue;
                            }
                            pending.push(Pending::CompleteTable(key, targets));
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
                Some(Pending::CompleteNode(key, target)) => {
                    self.validated_call_targets
                        .borrow_mut()
                        .nodes
                        .insert(key, target.clone());
                }
                Some(Pending::CompleteTable(key, targets)) => {
                    self.validated_call_targets
                        .borrow_mut()
                        .tables
                        .insert(key, targets.clone());
                }
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
