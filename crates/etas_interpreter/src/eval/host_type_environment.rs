use etas_types::{TypeId, TypeStore, TypeSubstitutionEngine, ty::CanonicalTypeParameterIdentity};
use std::collections::HashMap;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct HostTypeEnvironment<'a> {
    parent: Option<&'a HostTypeEnvironment<'a>>,
    bindings: HashMap<String, TypeId>,
    parameters: HashMap<CanonicalTypeParameterIdentity, TypeId>,
    enum_layouts: Option<&'a HashMap<TypeId, etas_types::EnumLayoutFact>>,
    json_value: Option<TypeId>,
    pub(super) source_map: Option<TypeId>,
}

impl<'a> HostTypeEnvironment<'a> {
    pub(super) fn with_checked_types(checked: &'a etas_frontend::CheckedProject) -> Self {
        Self {
            enum_layouts: Some(&checked.types.enum_layouts),
            json_value: checked.types.known_std_types.json_value,
            source_map: checked
                .hir
                .source_standard_types
                .then(|| super::resolve_std_type(checked, &["std", "collections", "Map"]))
                .flatten(),
            ..Self::default()
        }
    }

    pub(super) fn is_json_value(&self, ty: TypeId) -> bool {
        self.json_value == Some(ty)
    }

    pub(super) fn enum_layout(&self, ty: TypeId) -> Result<&'a etas_types::EnumLayoutFact, String> {
        self.enum_layouts
            .and_then(|layouts| layouts.get(&ty))
            .ok_or_else(|| format!("checked enum layout for {ty:?} is missing"))
    }
    pub(super) fn concrete_type(
        &self,
        ty: TypeId,
        store: &etas_types::TypeStore,
    ) -> Result<TypeId, String> {
        if self.parent.is_none() {
            return Ok(ty);
        }
        TypeSubstitutionEngine::existing(store, &self.bindings)
            .with_canonical_parameters(&self.parameters)
            .substitute(ty)
            .map_err(|error| {
                format!("host ABI type was not materialized by the checked pipeline: {error}")
            })
    }
    pub(super) fn applied(
        parent: &'a Self,
        store: &TypeStore,
        owner: TypeId,
        names: &[String],
        args: &[TypeId],
    ) -> Result<Self, String> {
        let args = args
            .iter()
            .map(|arg| parent.concrete_type(*arg, store))
            .collect::<Result<Vec<_>, _>>()?;
        let parameters = etas_types::ty::canonical_parameters_for_application(store, owner, &args)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            parent: Some(parent),
            bindings: names.iter().cloned().zip(args).collect(),
            parameters,
            enum_layouts: parent.enum_layouts,
            json_value: parent.json_value,
            source_map: parent.source_map,
        })
    }

    pub(super) fn lookup(
        &self,
        name: &str,
        identity: Option<&CanonicalTypeParameterIdentity>,
    ) -> Option<(TypeId, &Self)> {
        let mut frame = self;
        while let Some(parent) = frame.parent {
            let binding = match identity {
                Some(identity) => frame.parameters.get(identity),
                None => frame.bindings.get(name),
            };
            if let Some(ty) = binding {
                // An argument is interpreted in the caller's scope, never in
                // the scope of the parameter to which it has just been bound.
                return Some((*ty, parent));
            }
            frame = parent;
        }
        None
    }
}
