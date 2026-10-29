use crate::value::{CodeMetadataPayload, CodeMetadataValue, InterpValue as V, RecordValue};
use etas_types::{Type, TypeId, TypeStore, facts::symbols::KnownCodeTypes};

pub(super) struct Projector<'a> {
    pub store: &'a TypeStore,
    pub known: &'a KnownCodeTypes,
}

impl Projector<'_> {
    fn fields(&self, ty: TypeId) -> Result<&etas_types::RecordType, String> {
        let Some(Type::Nominal(nominal)) = self.store.get(ty) else {
            return Err("compiler projection requires a checked nominal type".into());
        };
        match nominal.representation.and_then(|ty| self.store.get(ty)) {
            Some(Type::Record(record)) => Ok(record),
            _ => Err("compiler projection has no checked record representation".into()),
        }
    }
    pub fn field(&self, ty: TypeId, name: &str) -> Result<TypeId, String> {
        self.fields(ty)?
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.ty)
            .ok_or_else(|| format!("compiler projection is missing checked field `{name}`"))
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
            _ => Err("compiler projection requires a checked Option type".into()),
        }
    }
    pub fn record(&self, ty: TypeId, fields: Vec<(&str, V)>) -> Result<V, String> {
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
                .iter()
                .find(|(name, _)| *name == field.name)
                .ok_or("compiler projection is missing a checked field")?;
            self.validate_data_value(field.ty, &value.1)?;
        }
        Ok(V::Nominal {
            ty,
            value: V::Record(RecordValue::new(
                fields
                    .into_iter()
                    .map(|(name, value)| (name.into(), value))
                    .collect(),
            ))
            .into(),
        })
    }
    pub fn metadata(&self, ty: TypeId, payload: CodeMetadataPayload) -> Result<V, String> {
        CodeMetadataValue::new(self.known, ty, payload).map(V::CodeMetadata)
    }
    pub fn optional_metadata(
        &self,
        ty: TypeId,
        payload: Option<CodeMetadataPayload>,
    ) -> Result<V, String> {
        let item = self.option(ty)?;
        payload
            .map(|payload| self.metadata(item, payload))
            .transpose()
            .map(optional)
    }

    fn validate_data_value(&self, ty: TypeId, value: &V) -> Result<(), String> {
        let mut pending = vec![(ty, value.clone())];
        while let Some((ty, value)) = pending.pop() {
            match (self.store.get(ty), value) {
                (Some(Type::Primitive(etas_types::PrimitiveType::String)), V::String(_))
                | (Some(Type::Primitive(etas_types::PrimitiveType::Bool)), V::Bool(_)) => {}
                (Some(Type::Primitive(expected)), V::Number(value))
                    if value.primitive() == *expected => {}
                (Some(Type::Nominal(_)), V::CodeMetadata(value)) if value.ty() == ty => {}
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
                .is_err()
        );
    }
}
