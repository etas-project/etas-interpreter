use etas_types::TypeId;
use std::collections::HashMap;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct HostTypeEnvironment<'a> {
    parent: Option<&'a HostTypeEnvironment<'a>>,
    bindings: HashMap<String, TypeId>,
    enum_layouts: Option<&'a HashMap<TypeId, etas_types::EnumLayoutFact>>,
    json_value: Option<TypeId>,
}

impl<'a> HostTypeEnvironment<'a> {
    pub(super) fn with_checked_types(facts: &'a etas_types::TypeFacts) -> Self {
        Self {
            enum_layouts: Some(&facts.enum_layouts),
            json_value: facts.known_std_types.json_value,
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
        let Some(parent) = self.parent else {
            return Ok(ty);
        };
        let mut bindings = HashMap::new();
        for (name, arg) in &self.bindings {
            bindings.insert(name.clone(), parent.concrete_type(*arg, store)?);
        }
        etas_types::substitute_named_params_in_store(store, ty, &bindings).map_err(|error| {
            format!("host ABI type was not materialized by the checked pipeline: {error}")
        })
    }
    pub(super) fn applied(parent: &'a Self, names: &[String], args: &[TypeId]) -> Self {
        Self {
            parent: Some(parent),
            bindings: names.iter().cloned().zip(args.iter().copied()).collect(),
            enum_layouts: parent.enum_layouts,
            json_value: parent.json_value,
        }
    }

    pub(super) fn lookup(&self, name: &str) -> Option<(TypeId, &Self)> {
        let mut frame = self;
        while let Some(parent) = frame.parent {
            if let Some(ty) = frame.bindings.get(name) {
                // An argument is interpreted in the caller's scope, never in
                // the scope of the parameter to which it has just been bound.
                return Some((*ty, parent));
            }
            frame = parent;
        }
        None
    }
}
