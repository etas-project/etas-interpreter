use super::{CallTargetSnapshot as Target, SnapshotValidator};
use etas_types::{FlowType, Type, TypeId};
use std::{collections::HashMap, rc::Rc};

mod leaf;
#[cfg(test)]
mod tests;

#[derive(Clone)]
struct Interface {
    signature: Option<CallableAbi>,
    component: Option<TypeId>,
}

#[derive(Clone)]
struct CallableAbi {
    input: Vec<TypeId>,
    output: TypeId,
}

impl From<FlowType> for CallableAbi {
    fn from(signature: FlowType) -> Self {
        Self {
            input: signature.input,
            output: signature.output,
        }
    }
}

type Bindings = crate::control::GenericBindings;
type BindingKey = (
    Vec<(String, TypeId)>,
    Vec<(String, etas_types::EffectRowRef)>,
);
type CacheKey = (u8, usize, BindingKey);
type RootCacheKey = (CacheKey, TypeId, BindingKey);

fn binding_key(bindings: &Bindings) -> BindingKey {
    let mut types = bindings
        .iter()
        .map(|(name, ty)| (name.clone(), *ty))
        .collect::<Vec<_>>();
    types.sort_unstable();
    (types, bindings.sorted_rows())
}

#[derive(Default)]
pub(super) struct TypedCallables {
    // Pin the edge used as a cache key, including its checked type and generic
    // environment. COW mutation cannot reuse an older successful result.
    roots: HashMap<RootCacheKey, Target>,
}

fn root_key(target: &Target, expected: TypeId, bindings: &Bindings) -> Option<RootCacheKey> {
    let (kind, pointer) = match target {
        Target::Limited { target, .. } => (0, target.shared_identity()? as usize),
        Target::Specialized { target, .. } => (1, target.shared_identity()? as usize),
        Target::Composed(targets) => (2, targets.shared_identity()? as usize),
        _ => return None,
    };
    let names = binding_key(bindings);
    let mut specialization = match target {
        Target::Specialized {
            type_bindings,
            effect_row_bindings,
            ..
        } => (type_bindings.clone(), effect_row_bindings.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    specialization.0.sort_unstable();
    specialization.1.sort_unstable();
    Some(((kind, pointer, names), expected, specialization))
}

enum Task<'a> {
    Visit(&'a Target, Rc<Bindings>),
    Cache(CacheKey),
    Compose(usize),
}

impl SnapshotValidator<'_> {
    pub(super) fn reference_target_type(
        &self,
        root: &Target,
        expected: TypeId,
    ) -> Result<(), String> {
        self.reference_target_type_with_bindings(root, expected, &Bindings::new())
    }

    pub(super) fn reference_target_type_with_bindings(
        &self,
        root: &Target,
        expected: TypeId,
        bindings: &Bindings,
    ) -> Result<(), String> {
        let key = root_key(root, expected, bindings);
        if key
            .as_ref()
            .is_some_and(|key| self.typed_callables.borrow().roots.contains_key(key))
        {
            return Ok(());
        }
        let signature = match self.checked.type_store.get(expected) {
            Some(Type::Function(signature)) => {
                Some(leaf::substitute(self, signature.clone(), bindings)?)
            }
            Some(_) => None,
            None => return Err("checkpoint callable slot type is missing".into()),
        };
        let allowed = signature
            .as_ref()
            .and_then(|signature| signature.effects.clone());
        // Shared compositions are DAGs. Cache by immutable edge and binding
        // environment, not just target ID, and never recursively walk the DAG.
        let mut cache: HashMap<CacheKey, Interface> = HashMap::new();
        let mut results = Vec::new();
        let mut pending = vec![Task::Visit(root, Rc::new(Bindings::new()))];
        while let Some(task) = pending.pop() {
            match task {
                Task::Visit(target, bindings) => {
                    let key = match target {
                        Target::Limited { target, .. } | Target::Specialized { target, .. } => {
                            target.shared_identity().map(|key| (0, key as usize))
                        }
                        Target::Composed(targets) => {
                            targets.shared_identity().map(|key| (1, key as usize))
                        }
                        _ => None,
                    };
                    let next_bindings = match target {
                        Target::Specialized {
                            type_bindings,
                            effect_row_bindings,
                            ..
                        } => {
                            let mut combined = (*bindings).clone();
                            combined.extend(type_bindings.iter().cloned());
                            combined.rows.extend(effect_row_bindings.iter().cloned());
                            Rc::new(combined)
                        }
                        _ => bindings,
                    };
                    if let Some((kind, pointer)) = key {
                        let names = binding_key(&next_bindings);
                        let key = (kind, pointer, names);
                        if let Some(interface) = cache.get(&key) {
                            results.push(interface.clone());
                            continue;
                        }
                        pending.push(Task::Cache(key));
                    }
                    match target {
                        Target::Limited { target, .. } | Target::Specialized { target, .. } => {
                            pending.push(Task::Visit(target, next_bindings))
                        }
                        Target::Composed(targets) => {
                            if targets.is_empty() {
                                return Err("checkpoint callable composition is empty".into());
                            }
                            pending.push(Task::Compose(targets.len()));
                            pending.extend(
                                targets
                                    .iter()
                                    .rev()
                                    .map(|target| Task::Visit(target, next_bindings.clone())),
                            );
                        }
                        leaf => results.push(self.callable_leaf_interface(
                            leaf,
                            &next_bindings,
                            &allowed,
                        )?),
                    }
                }
                Task::Cache(key) => {
                    let interface = results
                        .last()
                        .ok_or("checkpoint callable interface stack is empty")?;
                    cache.insert(key, interface.clone());
                }
                Task::Compose(count) => {
                    let offset = results
                        .len()
                        .checked_sub(count)
                        .ok_or("checkpoint callable composition is incomplete")?;
                    let stages = &results[offset..];
                    let mut signature = stages[0]
                        .signature
                        .clone()
                        .ok_or("checkpoint composition stage is not callable")?;
                    for stage in &stages[1..] {
                        let next = stage
                            .signature
                            .as_ref()
                            .ok_or("checkpoint composition stage is not callable")?;
                        if next.input != [signature.output] {
                            return Err("checkpoint composition stages have incompatible checked interfaces".into());
                        }
                        signature.output = next.output;
                    }
                    results.truncate(offset);
                    results.push(Interface {
                        signature: Some(signature),
                        component: None,
                    });
                }
            }
        }
        let interface = results
            .pop()
            .ok_or("checkpoint callable has no checked interface")?;
        let matches = match signature {
            Some(expected) => {
                interface.component.is_none()
                    && interface.signature.as_ref().is_some_and(|actual| {
                        actual.input == expected.input && actual.output == expected.output
                    })
            }
            None => {
                let component = etas_types::TypeSubstitutionEngine::existing(
                    &self.checked.type_store,
                    bindings,
                )
                .substitute(expected)
                .map_err(|error| error.to_string())?;
                interface.component == Some(component)
            }
        };
        if !matches {
            return Err("checkpoint callable differs from its checked slot type".into());
        }
        if let Some(key) = key {
            self.typed_callables
                .borrow_mut()
                .roots
                .insert(key, root.clone());
        }
        Ok(())
    }

    fn snapshot_callable_specs(&self) -> &etas_types::SpecFacts {
        self.callable_specs
            .get_or_init(|| etas_types::SpecFacts::from(self.checked.types.as_ref()))
    }
}
