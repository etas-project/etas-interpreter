use std::{collections::HashMap, rc::Rc};

use crate::{
    control::{CallTarget as Target, GenericBindings as Bindings},
    plan::InterpreterPlan,
};
use etas_frontend::CheckedProject;
use etas_types::{EffectRowRef, FlowType, PrimitiveType, Type, TypeId};

#[derive(Clone, Copy)]
pub(super) enum Expected {
    Value(TypeId),
    Comparison(TypeId),
    Predicate(TypeId),
}

#[derive(Clone)]
struct Interface {
    signature: Option<FlowType>,
    component: Option<TypeId>,
}

type CacheKey = (
    u8,
    usize,
    Vec<(String, TypeId)>,
    Vec<(String, EffectRowRef)>,
);
enum Task<'a> {
    Visit(&'a Target, Rc<Bindings>),
    Cache(CacheKey),
    Compose(usize),
}

pub(super) fn validate(
    checked: &CheckedProject,
    plan: &InterpreterPlan,
    root: &Target,
    expected: Expected,
) -> Result<(), String> {
    let expected_signature = match expected {
        Expected::Value(ty) => match checked.type_store.get(ty) {
            Some(Type::Function(signature)) => Some(signature.clone()),
            Some(_) => None,
            None => return Err("callable input has no checked slot type".into()),
        },
        _ => None,
    };
    let allowed = match expected {
        Expected::Comparison(_) | Expected::Predicate(_) => Some(EffectRowRef {
            effects: Vec::new(),
            tail: None,
        }),
        Expected::Value(_) => expected_signature
            .as_ref()
            .and_then(|signature| signature.effects.clone()),
    };
    let specs = plan.checked_spec_facts();
    let mut cache = HashMap::<CacheKey, Interface>::new();
    let mut results = Vec::new();
    let mut pending = vec![Task::Visit(root, Rc::new(Bindings::new()))];
    while let Some(task) = pending.pop() {
        match task {
            Task::Visit(target, bindings) => {
                let edge = match target {
                    Target::Limited { target, .. } | Target::Specialized { target, .. } => target
                        .shared_identity()
                        .map(|pointer| (0, pointer as usize)),
                    Target::Composed(targets) => targets
                        .shared_identity()
                        .map(|pointer| (1, pointer as usize)),
                    _ => None,
                };
                let bindings = if let Target::Specialized {
                    type_bindings,
                    effect_row_bindings,
                    ..
                } = target
                {
                    let mut bindings = (*bindings).clone();
                    bindings.extend(type_bindings.iter().cloned());
                    bindings.rows.extend(effect_row_bindings.iter().cloned());
                    Rc::new(bindings)
                } else {
                    bindings
                };
                if let Some((kind, pointer)) = edge {
                    let mut types = bindings
                        .iter()
                        .map(|(name, ty)| (name.clone(), *ty))
                        .collect::<Vec<_>>();
                    types.sort_unstable();
                    let key = (kind, pointer, types, bindings.sorted_rows());
                    if let Some(interface) = cache.get(&key) {
                        results.push(interface.clone());
                        continue;
                    }
                    pending.push(Task::Cache(key));
                }
                match target {
                    Target::Limited { target, .. } | Target::Specialized { target, .. } => {
                        pending.push(Task::Visit(target, bindings))
                    }
                    Target::Composed(targets) => {
                        if targets.is_empty() {
                            return Err("callable input has an empty composition".into());
                        }
                        pending.push(Task::Compose(targets.len()));
                        pending.extend(
                            targets
                                .iter()
                                .rev()
                                .map(|target| Task::Visit(target, bindings.clone())),
                        );
                    }
                    leaf => results.push(leaf::interface(
                        checked, plan, leaf, &bindings, &allowed, specs,
                    )?),
                }
            }
            Task::Cache(key) => {
                cache.insert(
                    key,
                    results
                        .last()
                        .ok_or("callable input has an incomplete interface")?
                        .clone(),
                );
            }
            Task::Compose(count) => {
                let offset = results
                    .len()
                    .checked_sub(count)
                    .ok_or("callable composition is incomplete")?;
                let stages = &results[offset..];
                let mut signature = stages[0]
                    .signature
                    .clone()
                    .ok_or("composition stage is not callable")?;
                for stage in &stages[1..] {
                    let next = stage
                        .signature
                        .as_ref()
                        .ok_or("composition stage is not callable")?;
                    if next.input != [signature.output] {
                        return Err("callable composition has incompatible stage types".into());
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
    let actual = results
        .pop()
        .ok_or("callable input has no checked interface")?;
    let matches = match expected {
        Expected::Comparison(key) => {
            actual.component.is_none()
                && actual.signature.as_ref().is_some_and(|signature| {
                    signature.input == [key, key]
                        && matches!(
                            checked.type_store.get(signature.output),
                            Some(Type::Primitive(PrimitiveType::I32))
                        )
                })
        }
        Expected::Predicate(input) => {
            actual.component.is_none()
                && actual.signature.as_ref().is_some_and(|signature| {
                    signature.input == [input]
                        && matches!(
                            checked.type_store.get(signature.output),
                            Some(Type::Primitive(PrimitiveType::Bool))
                        )
                })
        }
        Expected::Value(expected) => match expected_signature {
            Some(signature) => {
                actual.component.is_none()
                    && actual.signature.as_ref().is_some_and(|actual| {
                        actual.input == signature.input && actual.output == signature.output
                    })
            }
            None => actual.component == Some(expected),
        },
    };
    matches
        .then_some(())
        .ok_or_else(|| "callable input differs from its checked slot interface".into())
}

mod leaf;
