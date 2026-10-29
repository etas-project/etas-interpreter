use std::collections::HashMap;

use etas_types::Type;

use super::{CheckedTypeCatalog, CheckedTypeIdentity};

pub struct CheckedVariantLayout {
    pub fields: Vec<CheckedTypeIdentity>,
    pub names: Option<Vec<String>>,
}

impl CheckedTypeCatalog {
    pub(crate) fn variant(
        &self,
        identity: &CheckedTypeIdentity,
        name: &str,
    ) -> Result<CheckedVariantLayout, String> {
        self.validate_identity(identity)?;
        let declaration = identity
            .declaration()
            .ok_or("variant has no checked enum owner")?;
        let owner = self.declaration_template(declaration)?;
        if !matches!(self.program.type_store.get(owner), Some(Type::Enum(_))) {
            return Err("variant owner is not an enum declaration".into());
        }
        let layout = self
            .program
            .types
            .enum_layouts
            .get(&owner)
            .ok_or("enum has no checked constructor layout")?;
        let variant = layout
            .variants
            .iter()
            .find(|variant| variant.name == name)
            .ok_or("unknown checked enum constructor")?;
        let bindings = self.bind_members(identity, &layout.type_params)?;
        let fields = variant
            .fields
            .iter()
            .map(|template| self.resolve(*template, &bindings, &Default::default()))
            .collect::<Result<_, _>>()?;
        Ok(CheckedVariantLayout {
            fields,
            names: variant.field_names.clone(),
        })
    }

    pub(crate) fn representation(
        &self,
        identity: &CheckedTypeIdentity,
    ) -> Result<CheckedTypeIdentity, String> {
        self.validate_identity(identity)?;
        let declaration = identity
            .declaration()
            .ok_or("value has no checked nominal declaration")?;
        let owner = self.declaration_template(declaration)?;
        let Some(Type::Nominal(nominal)) = self.program.type_store.get(owner) else {
            return Err("declaration is not a represented nominal type".into());
        };
        let template = nominal
            .representation
            .ok_or("opaque nominal type has no checked representation")?;
        let bindings = self.bind_members(identity, &nominal.params)?;
        self.resolve(template, &bindings, &Default::default())
    }

    pub(super) fn bind_members(
        &self,
        identity: &CheckedTypeIdentity,
        parameters: &[String],
    ) -> Result<HashMap<String, CheckedTypeIdentity>, String> {
        if parameters.len() != identity.arguments().len() {
            return Err("nominal identity has an inconsistent generic arity".into());
        }
        let mut bindings = HashMap::new();
        for (name, ty) in parameters.iter().zip(identity.arguments()) {
            if bindings.insert(name.clone(), ty.clone()).is_some() {
                return Err("nominal declaration has duplicate type parameters".into());
            }
        }
        Ok(bindings)
    }
}
