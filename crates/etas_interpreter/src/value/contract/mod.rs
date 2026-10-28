mod callable;
mod native;
pub(crate) mod runtime;
#[cfg(test)]
mod tests;
mod view;

use etas_frontend::CheckedProject;
use etas_types::{PrimitiveType as P, Type, TypeId};
use std::collections::{HashMap, HashSet};
use view::{Identity, MapKind, Node, SequenceKind};
pub(crate) use view::{Target, Value};

pub(crate) trait Context {
    fn checked(&self) -> &CheckedProject;
    fn limits(&self) -> &etas_host::StorageLimits;
    fn callable(&self, value: Target<'_>, expected: TypeId) -> Result<(), String>;
    fn comparator(&self, value: Target<'_>, key: TypeId) -> Result<(), String>;
    fn predicate(&self, value: Target<'_>, input: TypeId) -> Result<(), String>;
    fn native(&self, value: Value<'_>, expected: TypeId) -> Result<(), String>;
}

#[derive(Clone, Copy)]
pub(crate) struct Seed<'a> {
    pub value: Value<'a>,
    pub ty: TypeId,
}
impl<'a> Seed<'a> {
    fn new(value: Value<'a>, ty: TypeId) -> Self {
        Self { value, ty }
    }
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key<'a>(Identity<'a>, TypeId, bool);
enum Task<'a> {
    Visit(Seed<'a>, bool),
    Complete(Key<'a>),
    Children(Box<dyn Iterator<Item = Seed<'a>> + 'a>),
}

pub(crate) fn validate<'a>(
    context: &'a impl Context,
    values: impl IntoIterator<Item = Seed<'a>>,
    bindings: &crate::control::GenericBindings,
) -> Result<(), String> {
    let checked = context.checked();
    let store = &checked.type_store;
    let mut substitution = bindings.substitution(store);
    let mut layouts = crate::plan::code::CheckedValueTypes::default();
    let mut records = HashMap::<TypeId, HashMap<&str, TypeId>>::new();
    let mut completed = HashSet::new();
    let mut active = HashSet::new();
    let mut pending = values
        .into_iter()
        .map(|seed| Task::Visit(seed, false))
        .collect::<Vec<_>>();
    while let Some(task) = pending.pop() {
        let (value, expected, enum_payload) = match task {
            Task::Complete(key) => {
                active.remove(&key);
                completed.insert(key);
                continue;
            }
            Task::Children(mut values) => {
                if let Some(seed) = values.next() {
                    pending.push(Task::Children(values));
                    pending.push(Task::Visit(seed, false));
                }
                continue;
            }
            Task::Visit(seed, flag) => (
                seed.value,
                substitution
                    .substitute(seed.ty)
                    .map_err(|error| format!("input type is not materialized: {error}"))?,
                flag,
            ),
        };
        let mut expected = expected;
        let mut refinements = HashSet::new();
        while let Some(Type::Refined { base, .. }) = store.get(expected) {
            if !refinements.insert(expected) {
                return Err("cyclic checked refinement type".into());
            }
            expected = *base;
        }
        let ty = store
            .get(expected)
            .ok_or("input type is missing from checked facts")?;
        if let Some(comparator) = value.comparator()? {
            let Type::Applied { args, .. } = ty else {
                return Err("ordered value has no checked generic arguments".into());
            };
            let role = checked.types.known_std_types.runtime_role(store, expected);
            let key = match (role, args.as_slice()) {
                (Some(etas_std::RuntimeTypeRole::PriorityQueue), [_, key])
                | (Some(etas_std::RuntimeTypeRole::OrderedMap), [key, _])
                | (Some(etas_std::RuntimeTypeRole::OrderedSet), [key]) => *key,
                _ => return Err("comparator has no checked collection identity".into()),
            };
            context.comparator(comparator, key)?;
        }
        if let Some(identity) = value.identity() {
            let key = Key(identity, expected, enum_payload);
            if completed.contains(&key) {
                continue;
            }
            if !active.insert(key.clone()) {
                return Err("cyclic runtime value representation".into());
            }
            pending.push(Task::Complete(key));
        }
        match (ty, value.node()) {
            (Type::Primitive(P::Unit), Node::Unit)
            | (Type::Primitive(P::Bool), Node::Bool)
            | (Type::Primitive(P::String), Node::String(_))
            | (Type::Primitive(P::Bytes), Node::Bytes) => {}
            (Type::Primitive(P::Char), Node::String(text)) if text.chars().count() == 1 => {}
            (Type::Primitive(primitive), Node::Number(number))
                if *primitive == number.primitive() => {}
            (_, Node::Nominal(actual, inner)) if actual == expected => {
                if is_enum(store, expected) {
                    pending.push(Task::Visit(Seed::new(inner, expected), true));
                } else {
                    let representation =
                        layouts.representation(checked, expected).map_err(|error| {
                            format!("nominal input has no checked representation: {error:?}")
                        })?;
                    pending.push(Task::Visit(Seed::new(inner, representation), false));
                }
            }
            (Type::Trust { wrapper, inner }, Node::Trust(actual, value)) if wrapper == &actual => {
                pending.push(Task::Visit(Seed::new(value, *inner), false))
            }
            (Type::Option(_), Node::None) => {}
            (Type::Option(inner), Node::Some(value)) => {
                pending.push(Task::Visit(Seed::new(value, *inner), false))
            }
            (Type::Array(inner), Node::Sequence(SequenceKind::Array, _, values))
            | (Type::List(inner), Node::Sequence(SequenceKind::List, _, values))
            | (Type::Slice(inner), Node::Sequence(SequenceKind::Slice, _, values))
            | (Type::Set(inner), Node::Sequence(SequenceKind::Set, _, values)) => {
                let inner = *inner;
                pending.push(Task::Children(Box::new(
                    values.map(move |value| Seed::new(value, inner)),
                )));
            }
            (Type::Tuple(types), Node::Sequence(SequenceKind::Tuple, len, values))
                if types.len() == len =>
            {
                pending.push(Task::Children(Box::new(
                    values
                        .zip(types.iter().copied())
                        .map(|(value, ty)| Seed::new(value, ty)),
                )));
            }
            (Type::Record(record), Node::Record(len, fields)) if record.fields.len() == len => {
                let types = records.entry(expected).or_insert_with(|| {
                    record
                        .fields
                        .iter()
                        .map(|field| (field.name.as_str(), field.ty))
                        .collect()
                });
                if types.len() != len {
                    return Err("checked record has duplicate field names".into());
                }
                let mut names = HashSet::new();
                let mut children = Vec::with_capacity(len);
                for (name, value) in fields {
                    if !names.insert(name) {
                        return Err("input record has duplicate fields".into());
                    }
                    let ty = *types.get(name).ok_or("input record has an unknown field")?;
                    children.push(Seed::new(value, ty));
                }
                pending.push(Task::Children(Box::new(children.into_iter())));
            }
            (Type::Map { key, value }, Node::Map(MapKind::Map, entries)) => {
                let (key, value) = (*key, *value);
                pending.push(Task::Children(Box::new(
                    entries.flat_map(move |(a, b)| [Seed::new(a, key), Seed::new(b, value)]),
                )));
            }
            (Type::Range { index }, Node::Range(a, b)) => {
                pending.push(Task::Visit(Seed::new(a, *index), false));
                pending.push(Task::Visit(Seed::new(b, *index), false));
            }
            (Type::Result { ok, err }, Node::Variant(name, 1, mut fields))
                if name == "Ok" || name == "Err" =>
            {
                let value = fields.next().ok_or("result input has no payload")?;
                pending.push(Task::Visit(
                    Seed::new(value, if name == "Ok" { *ok } else { *err }),
                    false,
                ));
            }
            (_, Node::Variant(name, len, fields)) if enum_payload && is_enum(store, expected) => {
                let variant = layouts.variant(checked, expected, name).map_err(|error| {
                    format!("input variant has no checked constructor layout: {error:?}")
                })?;
                if len != variant.fields.len() {
                    return Err(
                        "input variant payload arity differs from its checked constructor".into(),
                    );
                }
                let types = variant.fields.clone();
                pending.push(Task::Children(Box::new(
                    fields.zip(types).map(|(value, ty)| Seed::new(value, ty)),
                )));
            }
            (Type::Applied { args, .. }, Node::Sequence(SequenceKind::Native(role), _, values))
                if checked.types.known_std_types.runtime_role(store, expected) == Some(role)
                    && args.len() == 1 =>
            {
                let inner = args[0];
                pending.push(Task::Children(Box::new(
                    values.map(move |value| Seed::new(value, inner)),
                )));
            }
            (Type::Applied { args, .. }, Node::Map(MapKind::Native(role), values))
                if checked.types.known_std_types.runtime_role(store, expected) == Some(role)
                    && args.len() == 2 =>
            {
                let (key, value) = if role == etas_std::RuntimeTypeRole::PriorityQueue {
                    (args[1], args[0])
                } else {
                    (args[0], args[1])
                };
                pending.push(Task::Children(Box::new(
                    values.flat_map(move |(a, b)| [Seed::new(a, key), Seed::new(b, value)]),
                )));
            }
            (_, Node::Native) => native::validate(context, value, expected, &mut pending)?,
            _ => {
                return Err(format!(
                    "runtime input does not match checked type {}",
                    etas_types::ty::display_type(store, expected)
                ));
            }
        }
    }
    Ok(())
}

fn is_enum(store: &etas_types::TypeStore, ty: TypeId) -> bool {
    matches!(store.get(ty), Some(Type::Enum(_)))
        || matches!(store.get(ty), Some(Type::Applied { constructor, .. }) if matches!(store.get(TypeId(constructor.0)), Some(Type::Enum(_))))
}
