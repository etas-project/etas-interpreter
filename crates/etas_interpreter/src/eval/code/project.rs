use crate::value::{CodeMetadataPayload, CodeMetadataValue, InterpValue as V, RecordValue};
use etas_types::{Type, TypeId, TypeStore, facts::symbols::KnownCodeTypes};

pub(super) type EnumLayouts = std::collections::HashMap<TypeId, etas_types::EnumLayoutFact>;

pub(super) struct Projector<'a> {
    pub store: &'a TypeStore,
    pub known: &'a KnownCodeTypes,
    pub enums: &'a EnumLayouts,
}

impl Projector<'_> {
    pub(super) fn fields(&self, ty: TypeId) -> Result<etas_types::RecordType, String> {
        etas_types::record_fields_with_applied_params(self.store, ty)
            .map_err(|error| format!("compiler record projection failed: {error:?}"))?
            .ok_or_else(|| "compiler projection has no checked record representation".into())
    }
    pub fn field(&self, ty: TypeId, name: &str) -> Result<TypeId, String> {
        self.optional_field(ty, name)?
            .ok_or_else(|| format!("compiler projection is missing checked field `{name}`"))
    }
    pub fn optional_field(&self, ty: TypeId, name: &str) -> Result<Option<TypeId>, String> {
        Ok(self
            .fields(ty)?
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.ty))
    }
    pub fn array_element(&self, ty: TypeId) -> Result<TypeId, String> {
        match self.store.get(ty) {
            Some(Type::Array(item)) => Ok(*item),
            _ => Err("compiler projection requires a checked Array type".into()),
        }
    }
    pub fn option(&self, ty: TypeId) -> Result<TypeId, String> {
        match self.store.get(ty) {
            Some(Type::Option(item)) => Ok(*item),
            _ => self
                .source_option_element(ty)?
                .ok_or_else(|| "compiler projection requires a checked Option type".into()),
        }
    }
    fn source_option_element(&self, ty: TypeId) -> Result<Option<TypeId>, String> {
        let Some(Type::Applied { constructor, args }) = self.store.get(ty) else {
            return Ok(None);
        };
        let owner = TypeId(constructor.0);
        if !matches!(self.store.get(owner), Some(Type::Enum(declaration))
            if declaration.name == "std.option.Option")
        {
            return Ok(None);
        }
        let [element] = args.as_slice() else {
            return Err("compiler Option carrier has no checked element argument".into());
        };
        let layout = self
            .enums
            .get(&owner)
            .ok_or("compiler Option carrier has no checked enum layout")?;
        let [parameter] = layout.type_params.as_slice() else {
            return Err("compiler Option carrier has no checked element binder".into());
        };
        if !layout.variants.iter().any(|variant| {
            variant.name == "Some"
                && variant.field_names.is_none()
                && matches!(variant.fields.as_slice(), [field]
                    if matches!(self.store.get(*field), Some(Type::Named(binder)) if binder.name == *parameter))
        }) || !layout
            .variants
            .iter()
            .any(|variant| variant.name == "None" && variant.fields.is_empty())
        {
            return Err(
                "compiler Option carrier requires source Some(T) and None constructors".into(),
            );
        }
        Ok(Some(*element))
    }

    pub fn optional_value(&self, ty: TypeId, value: Option<V>) -> Result<V, String> {
        let element = self.option(ty)?;
        let value = value
            .map(|value| self.normalize(element, value))
            .transpose()?;
        if self.source_option_element(ty)?.is_none() {
            return Ok(optional(value));
        }
        let (name, fields) = match value {
            Some(value) => ("Some", vec![value]),
            None => ("None", Vec::new()),
        };
        Ok(V::Nominal {
            ty,
            value: V::Variant {
                name: name.into(),
                fields: fields.into(),
            }
            .into(),
        })
    }

    /// Native descriptions may use an internal optional marker while they are
    /// assembled. Convert it at the typed ABI boundary into the actual source
    /// enum; no builtin Option value escapes into source code.
    fn normalize(&self, ty: TypeId, value: V) -> Result<V, String> {
        if self.source_option_element(ty)?.is_some() {
            return match value {
                V::OptionNone => self.optional_value(ty, None),
                V::OptionSome(value) => self.optional_value(ty, Some(value.into_value())),
                value @ V::Nominal { ty: actual, .. } if actual == ty => Ok(value),
                _ => Err("compiler optional data differs from its checked source enum".into()),
            };
        }
        match (self.store.get(ty), value) {
            (Some(Type::Array(element)), V::Array(values)) => values
                .borrow()
                .iter()
                .cloned()
                .map(|value| self.normalize(*element, value))
                .collect::<Result<Vec<_>, _>>()
                .map(|values| V::Array(values.into())),
            (Some(Type::Tuple(elements)), V::Tuple(values)) if elements.len() == values.len() => {
                elements
                    .iter()
                    .zip(values.iter())
                    .map(|(ty, value)| self.normalize(*ty, value.clone()))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| V::Tuple(values.into()))
            }
            (_, value) => Ok(value),
        }
    }

    pub fn record(&self, ty: TypeId, mut fields: Vec<(&str, V)>) -> Result<V, String> {
        let expected = self.fields(ty)?;
        if expected.fields.len() != fields.len()
            || expected
                .fields
                .iter()
                .any(|field| !fields.iter().any(|(name, _)| *name == field.name))
        {
            return Err("compiler projection fields differ from the checked ABI".into());
        }
        for field in &expected.fields {
            let value = fields
                .iter_mut()
                .find(|(name, _)| *name == field.name)
                .ok_or("compiler projection is missing a checked field")?;
            value.1 = self.normalize(field.ty, value.1.clone())?;
            self.validate_data_value(field.ty, &value.1)?;
        }
        let value = V::Record(RecordValue::new(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        ));
        Ok(match self.store.get(ty) {
            Some(Type::Nominal(_) | Type::Applied { .. }) => V::Nominal {
                ty,
                value: value.into(),
            },
            _ => value,
        })
    }
    pub fn metadata(&self, ty: TypeId, payload: CodeMetadataPayload) -> Result<V, String> {
        match payload {
            CodeMetadataPayload::Definition(value) if self.is_record_data(ty) => {
                super::identity::definition(self, ty, &value)
            }
            CodeMetadataPayload::Callable(value)
                if matches!(self.store.get(ty), Some(Type::Enum(_))) =>
            {
                super::identity::callable(self, ty, &value)
            }
            CodeMetadataPayload::GenericOwner(value)
                if matches!(self.store.get(ty), Some(Type::Enum(_))) =>
            {
                super::identity::generic_owner(self, ty, &value)
            }
            CodeMetadataPayload::Agent(value) if self.is_record_data(ty) => self.record(
                ty,
                vec![(
                    "definition",
                    super::identity::definition(self, self.field(ty, "definition")?, &value)?,
                )],
            ),
            CodeMetadataPayload::Type { graph, root }
                if matches!(self.store.get(ty), Some(Type::Enum(_))) =>
            {
                super::description::Description::new(self, &graph).type_info(ty, root)
            }
            CodeMetadataPayload::EffectRow { graph, root } if self.is_record_data(ty) => {
                super::description::Description::new(self, &graph).row(ty, root, false)
            }
            CodeMetadataPayload::Actions { graph, root } if self.is_record_data(ty) => {
                super::description::Description::new(self, &graph).row(ty, root, true)
            }
            CodeMetadataPayload::Trace { graph, root } if self.is_record_data(ty) => {
                super::description::Description::new(self, &graph).trace_summary(ty, root)
            }
            CodeMetadataPayload::TraceConstraint { graph, value } if self.is_record_data(ty) => {
                super::description::Description::new(self, &graph).constraint(ty, &value)
            }
            CodeMetadataPayload::TraceWitness(value) if self.is_record_data(ty) => {
                super::description::witness(self, ty, &value)
            }
            CodeMetadataPayload::Detail(value)
                if matches!(self.store.get(ty), Some(Type::Enum(_))) =>
            {
                super::description::detail(self, ty, &value)
            }
            CodeMetadataPayload::StaticValue(value)
                if matches!(self.store.get(ty), Some(Type::Enum(_))) =>
            {
                super::description::annotation_value(self, ty, &value)
            }
            CodeMetadataPayload::Determinism(value)
                if matches!(self.store.get(ty), Some(Type::Enum(_))) =>
            {
                self.variant(
                    ty,
                    match value {
                        etas_code::descriptor::DeterminismInfo::Deterministic => "Deterministic",
                        etas_code::descriptor::DeterminismInfo::RuntimeMediated => {
                            "RuntimeMediated"
                        }
                        etas_code::descriptor::DeterminismInfo::NonDeterministic => {
                            "NonDeterministic"
                        }
                    },
                    Vec::new(),
                )
            }
            payload => CodeMetadataValue::new(self.known, ty, payload).map(V::CodeMetadata),
        }
    }

    fn is_record_data(&self, ty: TypeId) -> bool {
        match self.store.get(ty) {
            Some(Type::Record(_)) => true,
            Some(Type::Nominal(nominal)) => nominal
                .representation
                .is_some_and(|ty| matches!(self.store.get(ty), Some(Type::Record(_)))),
            _ => false,
        }
    }
    pub fn optional_metadata(
        &self,
        ty: TypeId,
        payload: Option<CodeMetadataPayload>,
    ) -> Result<V, String> {
        let item = self.option(ty)?;
        let value = payload
            .map(|payload| self.metadata(item, payload))
            .transpose()?;
        self.optional_value(ty, value)
    }

    pub fn variant_layout(
        &self,
        ty: TypeId,
        name: &str,
    ) -> Result<&etas_types::EnumVariantLayoutFact, String> {
        self.enums
            .get(&ty)
            .and_then(|layout| layout.variants.iter().find(|variant| variant.name == name))
            .ok_or_else(|| format!("compiler projection has no checked enum variant `{name}`"))
    }

    pub fn variant_payload(&self, ty: TypeId, name: &str) -> Result<TypeId, String> {
        match self.variant_layout(ty, name)?.fields.as_slice() {
            [field] => Ok(*field),
            _ => Err("compiler enum projection requires one checked payload field".into()),
        }
    }

    pub fn tuple_elements(&self, ty: TypeId) -> Result<&[TypeId], String> {
        match self.store.get(ty) {
            Some(Type::Tuple(elements)) => Ok(elements),
            _ => Err("compiler projection requires a checked tuple type".into()),
        }
    }

    pub fn variant(&self, ty: TypeId, name: &str, mut fields: Vec<V>) -> Result<V, String> {
        let variant = self.variant_layout(ty, name)?;
        if variant.fields.len() != fields.len() {
            return Err("compiler enum projection differs from its checked payload arity".into());
        }
        for (ty, value) in variant.fields.iter().zip(&mut fields) {
            *value = self.normalize(*ty, value.clone())?;
            self.validate_data_value(*ty, value)?;
        }
        Ok(V::Nominal {
            ty,
            value: V::Variant {
                name: name.into(),
                fields: fields.into(),
            }
            .into(),
        })
    }

    pub fn variant_field(&self, ty: TypeId, variant: &str, name: &str) -> Result<TypeId, String> {
        let layout = self.variant_layout(ty, variant)?;
        layout
            .field_names
            .as_ref()
            .and_then(|names| {
                names
                    .iter()
                    .zip(&layout.fields)
                    .find(|(field, _)| *field == name)
            })
            .map(|(_, ty)| *ty)
            .ok_or_else(|| format!("compiler enum projection has no checked field `{name}`"))
    }

    pub fn named_variant(
        &self,
        ty: TypeId,
        variant: &str,
        fields: Vec<(&str, V)>,
    ) -> Result<V, String> {
        let layout = self.variant_layout(ty, variant)?;
        let names = layout
            .field_names
            .as_ref()
            .ok_or("compiler enum projection has no named layout")?;
        if fields.len() != names.len() {
            return Err("compiler enum fields differ from its checked ABI".into());
        }
        let values = names
            .iter()
            .map(|name| {
                fields
                    .iter()
                    .find(|(field, _)| *field == name)
                    .map(|(_, value)| value.clone())
                    .ok_or_else(|| {
                        format!("compiler enum projection is missing checked field `{name}`")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.variant(ty, variant, values)
    }

    fn validate_data_value(&self, ty: TypeId, value: &V) -> Result<(), String> {
        let mut pending = vec![(ty, value.clone())];
        while let Some((ty, value)) = pending.pop() {
            if let Some(element) = self.source_option_element(ty)? {
                let V::Nominal { ty: actual, value } = value else {
                    return Err("compiler optional value has no source enum identity".into());
                };
                if actual != ty {
                    return Err(
                        "compiler optional value has a different source enum identity".into(),
                    );
                }
                match value.as_ref() {
                    V::Variant { name, fields } if name.as_str() == "None" && fields.is_empty() => {
                    }
                    V::Variant { name, fields } if name.as_str() == "Some" && fields.len() == 1 => {
                        pending.push((element, fields[0].clone()));
                    }
                    _ => {
                        return Err(
                            "compiler optional value differs from its checked source payload"
                                .into(),
                        );
                    }
                }
                continue;
            }
            match (self.store.get(ty), value) {
                (Some(Type::Primitive(etas_types::PrimitiveType::String)), V::String(_))
                | (Some(Type::Primitive(etas_types::PrimitiveType::Bool)), V::Bool(_)) => {}
                (Some(Type::Primitive(etas_types::PrimitiveType::Char)), V::String(value))
                    if value.chars().count() == 1 => {}
                (Some(Type::Primitive(expected)), V::Number(value))
                    if value.primitive() == *expected => {}
                (Some(Type::Nominal(_)), V::CodeMetadata(value)) if value.ty() == ty => {}
                (Some(Type::Nominal(owner)), V::AgentDefinition(value))
                    if owner.representation.is_none() && value.ty() == ty => {}
                (Some(Type::Nominal(owner)), V::CallableDefinition(value))
                    if owner.representation.is_none() && value.ty() == ty => {}
                (Some(Type::Nominal(owner)), V::SyntaxBlock(value))
                    if owner.representation.is_none() && value.ty() == ty => {}
                (Some(Type::Enum(_)), V::Nominal { ty: actual, value }) if actual == ty => {
                    let V::Variant { name, fields } = value.as_ref() else {
                        return Err("compiler enum projection has no constructor payload".into());
                    };
                    let variant = self.variant_layout(ty, name)?;
                    if variant.fields.len() != fields.len() {
                        return Err("compiler enum payload differs from its checked ABI".into());
                    }
                    pending.extend(variant.fields.iter().copied().zip(fields.iter().cloned()));
                }
                (Some(Type::Nominal(nominal)), V::Nominal { ty: actual, value })
                    if actual == ty =>
                {
                    pending.push((
                        nominal
                            .representation
                            .ok_or("compiler data nominal has no representation")?,
                        value.into_value(),
                    ));
                }
                (Some(Type::Record(expected)), V::Record(value)) => {
                    if expected.fields.len() != value.borrow().len() {
                        return Err(
                            "compiler record field count differs from its checked ABI".into()
                        );
                    }
                    for field in &expected.fields {
                        pending.push((
                            field.ty,
                            value
                                .get(&field.name)
                                .ok_or("compiler record is missing a checked field")?,
                        ));
                    }
                }
                (Some(Type::Array(element)), V::Array(values)) => {
                    pending.extend(
                        values
                            .borrow()
                            .iter()
                            .cloned()
                            .map(|value| (*element, value)),
                    );
                }
                (Some(Type::List(element)), V::List(values)) => {
                    pending.extend(values.iter().cloned().map(|value| (*element, value)));
                }
                (Some(Type::Tuple(elements)), V::Tuple(values))
                    if elements.len() == values.len() =>
                {
                    pending.extend(elements.iter().copied().zip(values.iter().cloned()));
                }
                (Some(Type::Option(_)), V::OptionNone) => {}
                (Some(Type::Option(element)), V::OptionSome(value)) => {
                    pending.push((*element, value.into_value()))
                }
                _ => return Err("compiler data value differs from its checked ABI".into()),
            }
        }
        Ok(())
    }
}

pub(super) fn optional(value: Option<V>) -> V {
    value
        .map(|value| V::OptionSome(value.into()))
        .unwrap_or(V::OptionNone)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compiler_record_projection_rejects_wrong_primitive_and_nominal_fields() {
        let mut types = etas_types::TypeInterner::new();
        let boolean = types.primitive(etas_types::PrimitiveType::Bool);
        let record = types.intern(Type::Record(etas_types::RecordType {
            fields: vec![etas_types::FieldType {
                name: "valid".into(),
                ty: boolean,
            }],
        }));
        let nominal = types.intern(Type::Nominal(etas_types::NominalTypeRef {
            read_only_erased_view: false,
            name: "test.Report".into(),
            params: Vec::new(),
            representation: Some(record),
        }));
        let known = KnownCodeTypes::default();
        let project = Projector {
            store: types.store(),
            known: &known,
            enums: &std::collections::HashMap::new(),
        };
        assert!(
            project
                .record(nominal, vec![("valid", V::Bool(true))])
                .is_ok()
        );
        assert!(
            project
                .record(nominal, vec![("valid", V::String("true".into()))])
                .is_err()
        );
        assert!(
            project
                .record(record, vec![("valid", V::Bool(true))])
                .is_ok()
        );
    }
}
