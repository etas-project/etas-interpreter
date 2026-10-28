use std::collections::{HashMap, HashSet};

use etas_frontend::CheckedProject;
use etas_types::{PrimitiveType, Type, TypeId};

use super::projection::enum_constructor;
use super::{Build, RevisionValueBridgeError, RevisionValueFailure as Failure, Walk, error, wrong};
use crate::value::InterpValue;

impl Walk<'_> {
    pub(super) fn step(
        &mut self,
        from: TypeId,
        to: TypeId,
        value: InterpValue,
    ) -> Result<(), RevisionValueBridgeError> {
        self.step_inner(from, to, value)
            .map_err(|failure| error(from, to, failure))
    }

    fn step_inner(&mut self, from: TypeId, to: TypeId, value: InterpValue) -> Result<(), Failure> {
        let from = unrefined(self.source, from)?;
        let to = unrefined(self.target, to)?;
        let source = self
            .source
            .type_store
            .get(from)
            .ok_or(Failure::MissingType)?;
        let target = self.target.type_store.get(to).ok_or(Failure::MissingType)?;
        if matches!(&value, InterpValue::CommandResult { .. }) {
            if self
                .source
                .types
                .known_std_types
                .runtime_type(etas_std::RuntimeTypeRole::CommandResult)
                != Some(from)
                || self
                    .target
                    .types
                    .known_std_types
                    .runtime_type(etas_std::RuntimeTypeRole::CommandResult)
                    != Some(to)
            {
                return Err(Failure::UnsupportedType);
            }
            self.values.push(value);
            return Ok(());
        }
        if let InterpValue::WorkspacePath(path) = value {
            let source_region = super::projection::workspace_path_region(self.source, from)?;
            let target_region = super::projection::workspace_path_region(self.target, to)?;
            if source_region != target_region || path.region.as_str() != source_region {
                return Err(Failure::InvalidField("workspace_path.region".into()));
            }
            self.values.push(InterpValue::WorkspacePath(path));
            return Ok(());
        }
        if let InterpValue::Callable(crate::control::CallTarget::AgentRevision(agent)) = value {
            let revision = self
                .revision
                .as_ref()
                .ok_or(Failure::ExecutableScopeRequired)?;
            let agent = revision
                .agent(self.source, self.target, from, to, agent)
                .map_err(Failure::InvalidCallable)?;
            self.values.push(InterpValue::Callable(
                crate::control::CallTarget::AgentRevision(agent),
            ));
            return Ok(());
        }
        if matches!(
            &value,
            InterpValue::AgentDefinition(_)
                | InterpValue::CallableDefinition(_)
                | InterpValue::SyntaxBlock(_)
                | InterpValue::CodeMetadata(_)
        ) {
            let value = self.code_value(from, to, value)?;
            self.values.push(value);
            return Ok(());
        }
        if let InterpValue::HostHandle(handle) = value {
            let opaque = |program: &CheckedProject, ty| {
                let constructor = match program.type_store.get(ty) {
                    Some(Type::Applied { constructor, .. }) => TypeId(constructor.0),
                    _ => ty,
                };
                program
                    .types
                    .known_std_types
                    .opaque_values
                    .contains(&constructor)
            };
            if !opaque(self.source, from) || !opaque(self.target, to) {
                return Err(Failure::UnsupportedType);
            }
            let actual = handle.nominal_type();
            if actual != from {
                return Err(Failure::WrongNominalIdentity { actual });
            }
            self.values
                .push(InterpValue::HostHandle(handle.into_revision_type(to)));
            return Ok(());
        }
        match (source, target) {
            (Type::Primitive(left), Type::Primitive(right)) if left == right => {
                primitive(*left, &value)?;
                self.values.push(value);
            }
            (Type::Array(left), Type::Array(right)) => {
                let InterpValue::Array(values) = value else {
                    return Err(wrong("Array", &value));
                };
                self.sequence(from, to, *left, *right, Build::Array, values.into_values());
            }
            (Type::List(left), Type::List(right)) => {
                let InterpValue::List(values) = value else {
                    return Err(wrong("List", &value));
                };
                self.sequence(from, to, *left, *right, Build::List, values.into_values());
            }
            (Type::Slice(left), Type::Slice(right)) => {
                let InterpValue::Slice(values) = value else {
                    return Err(wrong("Slice", &value));
                };
                self.sequence(from, to, *left, *right, Build::Slice, values.into_values());
            }
            (Type::Set(left), Type::Set(right)) => {
                let InterpValue::Set(values) = value else {
                    return Err(wrong("Set", &value));
                };
                self.sequence(from, to, *left, *right, Build::Set, values.into_values());
            }
            (Type::Map { key: a, value: b }, Type::Map { key: c, value: d }) => {
                let InterpValue::Map(entries) = value else {
                    return Err(wrong("Map", &value));
                };
                let children = entries
                    .into_values()
                    .into_iter()
                    .flat_map(|(key, value)| [(*a, *c, key), (*b, *d, value)])
                    .collect();
                self.children(from, to, Build::Map, children);
            }
            (Type::Tuple(left), Type::Tuple(right)) if left.len() == right.len() => {
                let InterpValue::Tuple(values) = value else {
                    return Err(wrong("Tuple", &value));
                };
                arity(left.len(), values.len())?;
                let children = left
                    .iter()
                    .zip(right)
                    .zip(values.into_values())
                    .map(|((a, b), value)| (*a, *b, value))
                    .collect();
                self.children(from, to, Build::Tuple, children);
            }
            (Type::Record(left), Type::Record(right)) => {
                let InterpValue::Record(values) = value else {
                    return Err(wrong("Record", &value));
                };
                arity(left.fields.len(), values.borrow().len())?;
                if left.fields.len() != right.fields.len() {
                    return Err(Failure::InconsistentLayout);
                }
                let mut source_fields: HashMap<_, _> = left
                    .fields
                    .iter()
                    .map(|field| (field.name.as_str(), field.ty))
                    .collect();
                let target_fields: HashMap<_, _> = right
                    .fields
                    .iter()
                    .map(|field| (field.name.as_str(), field.ty))
                    .collect();
                if source_fields.len() != left.fields.len()
                    || target_fields.len() != right.fields.len()
                {
                    return Err(Failure::InconsistentLayout);
                }
                let mut children = Vec::with_capacity(left.fields.len());
                let mut names = Vec::with_capacity(left.fields.len());
                for (name, value) in values.into_values() {
                    let source = source_fields
                        .remove(name.as_str())
                        .ok_or_else(|| Failure::InvalidField(name.clone()))?;
                    let to = *target_fields
                        .get(name.as_str())
                        .ok_or(Failure::InconsistentLayout)?;
                    names.push(name);
                    children.push((source, to, value));
                }
                self.children(from, to, Build::Record(names), children);
            }
            (Type::Option(left), Type::Option(right)) => match value {
                InterpValue::OptionNone => self.values.push(InterpValue::OptionNone),
                InterpValue::OptionSome(value) => self.children(
                    from,
                    to,
                    Build::Some,
                    vec![(*left, *right, value.into_value())],
                ),
                value => return Err(wrong("Option", &value)),
            },
            (Type::Result { ok: a, err: b }, Type::Result { ok: c, err: d }) => {
                let InterpValue::Variant { name, fields } = value else {
                    return Err(wrong("Result", &value));
                };
                arity(1, fields.len())?;
                let (left, right) = match name.as_str() {
                    "Ok" => (*a, *c),
                    "Err" => (*b, *d),
                    _ => return Err(Failure::UnknownVariant(name.to_string())),
                };
                self.sequence(
                    from,
                    to,
                    left,
                    right,
                    Build::Variant(name),
                    fields.into_values(),
                );
            }
            (
                Type::Trust {
                    wrapper: a,
                    inner: left,
                },
                Type::Trust {
                    wrapper: b,
                    inner: right,
                },
            ) if a == b => {
                let InterpValue::Trust { wrapper, value } = value else {
                    return Err(wrong("Trust", &value));
                };
                if wrapper != *a {
                    return Err(Failure::InconsistentLayout);
                }
                self.children(
                    from,
                    to,
                    Build::Trust(wrapper),
                    vec![(*left, *right, value.into_value())],
                );
            }
            (Type::Range { index: left }, Type::Range { index: right }) => {
                let InterpValue::Range(range) = value else {
                    return Err(wrong("Range", &value));
                };
                self.children(
                    from,
                    to,
                    Build::Range(range.bounds),
                    vec![(*left, *right, *range.start), (*left, *right, *range.end)],
                );
            }
            (Type::Schema(left), Type::Schema(right)) => {
                let InterpValue::Schema(actual) = value else {
                    return Err(wrong("Schema", &value));
                };
                if actual != *left {
                    return Err(Failure::WrongNominalIdentity { actual });
                }
                self.values.push(InterpValue::Schema(*right));
            }
            (Type::Message(left), Type::Message(right)) => {
                let InterpValue::Message(mut message) = value else {
                    return Err(wrong("Message", &value));
                };
                let payload =
                    std::mem::replace(&mut message.payload, InterpValue::Unit.into()).into_value();
                self.children(
                    from,
                    to,
                    Build::Message(message),
                    vec![(*left, *right, payload)],
                );
            }
            (Type::Prompt, Type::Prompt) => {
                if !matches!(value, InterpValue::Prompt(_)) {
                    return Err(wrong("Prompt", &value));
                }
                self.values.push(value);
            }
            (Type::Enum(_), Type::Enum(_)) | (Type::Applied { .. }, Type::Applied { .. })
                if enum_constructor(self.source, from).is_some()
                    && enum_constructor(self.target, to).is_some() =>
            {
                self.enum_value(from, to, value)?;
            }
            (Type::Nominal(_), Type::Nominal(_)) | (Type::Applied { .. }, Type::Applied { .. }) => {
                let InterpValue::Nominal { ty, value } = value else {
                    return Err(wrong("Nominal", &value));
                };
                if ty != from {
                    return Err(Failure::WrongNominalIdentity { actual: ty });
                }
                let left = self.source_types.representation(self.source, from)?;
                let right = self.target_types.representation(self.target, to)?;
                self.children(
                    from,
                    to,
                    Build::Nominal(to),
                    vec![(left, right, value.into_value())],
                );
            }
            (Type::Function(left), Type::Function(right)) => {
                let execution = self.execution.ok_or(Failure::ExecutableScopeRequired)?;
                let InterpValue::Callable(call) = value else {
                    return Err(wrong("Callable", &value));
                };
                if left.input.len() != right.input.len() {
                    return Err(Failure::InconsistentLayout);
                }
                let projection = super::CallableValueProjection::new(
                    self.source.clone(),
                    self.target.clone(),
                    from,
                    to,
                    left.clone(),
                    right.clone(),
                    self.revision.clone(),
                );
                let call = crate::value::ScopedCallableValue::project(execution, call, projection)
                    .map_err(Failure::InvalidCallable)?;
                self.values.push(InterpValue::Callable(call));
            }
            (Type::Handler(_), Type::Handler(_)) => {
                return Err(Failure::ExecutableScopeRequired);
            }
            _ => return Err(Failure::UnsupportedType),
        }
        Ok(())
    }

    fn sequence(
        &mut self,
        from: TypeId,
        to: TypeId,
        left: TypeId,
        right: TypeId,
        kind: Build,
        values: Vec<InterpValue>,
    ) {
        self.children(
            from,
            to,
            kind,
            values
                .into_iter()
                .map(|value| (left, right, value))
                .collect(),
        );
    }

    fn enum_value(&mut self, from: TypeId, to: TypeId, value: InterpValue) -> Result<(), Failure> {
        let InterpValue::Nominal { ty, value } = value else {
            return Err(wrong("Enum", &value));
        };
        if ty != from {
            return Err(Failure::WrongNominalIdentity { actual: ty });
        }
        let value = value.into_value();
        let InterpValue::Variant { name, fields } = value else {
            return Err(wrong("Enum", &value));
        };
        let left = self.source_types.variant(self.source, from, &name)?;
        let right = self.target_types.variant(self.target, to, &name)?;
        if left.names != right.names || left.fields.len() != right.fields.len() {
            return Err(Failure::InconsistentLayout);
        }
        arity(left.fields.len(), fields.len())?;
        let children = left
            .fields
            .iter()
            .zip(&right.fields)
            .zip(fields.into_values())
            .map(|((a, b), value)| (*a, *b, value))
            .collect();
        self.children(from, to, Build::Enum { ty: to, name }, children);
        Ok(())
    }
}

fn arity(expected: usize, actual: usize) -> Result<(), Failure> {
    if expected == actual {
        Ok(())
    } else {
        Err(Failure::WrongArity { expected, actual })
    }
}

fn unrefined(program: &CheckedProject, mut ty: TypeId) -> Result<TypeId, Failure> {
    let mut visited = HashSet::new();
    while let Some(Type::Refined { base, .. }) = program.type_store.get(ty) {
        if !visited.insert(ty) {
            return Err(Failure::CyclicType);
        }
        ty = *base;
    }
    Ok(ty)
}

fn primitive(ty: PrimitiveType, value: &InterpValue) -> Result<(), Failure> {
    let valid = match (ty, value) {
        (PrimitiveType::Unit, InterpValue::Unit)
        | (PrimitiveType::Bool, InterpValue::Bool(_))
        | (PrimitiveType::String, InterpValue::String(_))
        | (PrimitiveType::Bytes, InterpValue::Bytes(_)) => true,
        (PrimitiveType::Char, InterpValue::String(text)) => text.chars().count() == 1,
        (primitive, InterpValue::Number(value)) => primitive == value.primitive(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Failure::InvalidPrimitive)
    }
}
