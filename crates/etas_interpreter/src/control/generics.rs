use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};

use etas_types::{EffectRowRef, TypeId, TypeStore, TypeSubstitutionEngine};

#[derive(Clone, Debug, Default)]
pub(crate) struct EffectRowBindings(Option<std::sync::Arc<HashMap<String, EffectRowRef>>>);

impl PartialEq for EffectRowBindings {
    fn eq(&self, other: &Self) -> bool {
        self.deref() == other.deref()
    }
}
impl Eq for EffectRowBindings {}

impl EffectRowBindings {
    pub(crate) fn extend(&mut self, rows: impl IntoIterator<Item = (String, EffectRowRef)>) {
        let mut rows = rows.into_iter().peekable();
        if rows.peek().is_some() {
            self.deref_mut().extend(rows);
        }
    }
}

impl std::ops::Deref for EffectRowBindings {
    type Target = HashMap<String, EffectRowRef>;
    fn deref(&self) -> &Self::Target {
        static EMPTY: std::sync::LazyLock<HashMap<String, EffectRowRef>> =
            std::sync::LazyLock::new(HashMap::new);
        self.0.as_deref().unwrap_or(&EMPTY)
    }
}
impl std::ops::DerefMut for EffectRowBindings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        std::sync::Arc::make_mut(self.0.get_or_insert_with(Default::default))
    }
}
impl FromIterator<(String, EffectRowRef)> for EffectRowBindings {
    fn from_iter<T: IntoIterator<Item = (String, EffectRowRef)>>(iter: T) -> Self {
        let values: HashMap<_, _> = iter.into_iter().collect();
        Self((!values.is_empty()).then(|| std::sync::Arc::new(values)))
    }
}
impl<'a> IntoIterator for &'a EffectRowBindings {
    type Item = (&'a String, &'a EffectRowRef);
    type IntoIter = std::collections::hash_map::Iter<'a, String, EffectRowRef>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GenericBindings {
    pub(crate) types: HashMap<String, TypeId>,
    pub(crate) parameters: HashMap<TypeId, TypeId>,
    pub(crate) rows: EffectRowBindings,
}

pub(crate) struct CheckedCallBindings {
    pub types: Vec<(String, TypeId)>,
    pub parameters: Vec<(TypeId, TypeId)>,
    pub rows: Vec<(String, EffectRowRef)>,
}

impl CheckedCallBindings {
    pub(crate) fn resolve(
        checked: &etas_frontend::CheckedProgram,
        call: etas_hir::HirExprId,
        outer: &GenericBindings,
    ) -> Result<Self, String> {
        let fact = checked.types.generic_instantiations.get(&call);
        let mut substitution = outer.substitution(&checked.type_store);
        Ok(Self {
            types: fact
                .into_iter()
                .flat_map(|fact| &fact.type_bindings)
                .map(|(name, ty)| {
                    Ok((
                        name.clone(),
                        substitution
                            .substitute(*ty)
                            .map_err(|error| error.to_string())?,
                    ))
                })
                .collect::<Result<_, String>>()?,
            parameters: checked
                .types
                .checked_method_calls
                .get(&call)
                .or_else(|| {
                    checked
                        .types
                        .bound_method_values
                        .get(&call)
                        .map(|value| &value.method)
                })
                .into_iter()
                .flat_map(|method| &method.owner_bindings)
                .map(|(parameter, argument)| {
                    Ok((
                        *parameter,
                        substitution
                            .substitute(*argument)
                            .map_err(|error| error.to_string())?,
                    ))
                })
                .collect::<Result<_, String>>()?,
            rows: fact
                .into_iter()
                .flat_map(|fact| &fact.effect_row_bindings)
                .map(|(name, row)| {
                    Ok((
                        name.clone(),
                        substitution
                            .substitute_effect_row(row.clone())
                            .map_err(|error| error.to_string())?,
                    ))
                })
                .collect::<Result<_, String>>()?,
        })
    }
}

impl GenericBindings {
    pub(crate) fn is_checked_parameter(
        checked: &etas_frontend::CheckedProgram,
        parameter: TypeId,
    ) -> bool {
        checked.type_store.parameter_identity(parameter).is_some()
            || matches!(checked.type_store.get(parameter), Some(etas_types::Type::Named(named))
                if named.source_parameter.is_some_and(|symbol|
                    matches!(checked.hir.symbols.get(symbol).map(|symbol| &symbol.def), Some(etas_hir::SymbolDef::TypeParam { .. }))))
    }

    pub(crate) fn shared(self) -> std::sync::Arc<Self> {
        static EMPTY: std::sync::LazyLock<std::sync::Arc<GenericBindings>> =
            std::sync::LazyLock::new(Default::default);
        if self.is_empty() {
            EMPTY.clone()
        } else {
            std::sync::Arc::new(self)
        }
    }
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn substitution<'a>(&'a self, store: &'a TypeStore) -> TypeSubstitutionEngine<'a> {
        TypeSubstitutionEngine::existing(store, &self.types)
            .with_type_parameters(&self.parameters)
            .with_effect_arguments(&self.rows)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.types.is_empty() && self.parameters.is_empty() && self.rows.is_empty()
    }

    pub(crate) fn sorted_parameters(&self) -> Vec<(TypeId, TypeId)> {
        let mut parameters = self
            .parameters
            .iter()
            .map(|(parameter, ty)| (*parameter, *ty))
            .collect::<Vec<_>>();
        parameters.sort_unstable();
        parameters
    }

    pub(crate) fn sorted_types(&self) -> Vec<(String, TypeId)> {
        let mut types = self
            .types
            .iter()
            .map(|(name, ty)| (name.clone(), *ty))
            .collect::<Vec<_>>();
        types.sort_unstable();
        types
    }

    pub(crate) fn sorted_rows(&self) -> Vec<(String, EffectRowRef)> {
        let mut rows = self
            .rows
            .iter()
            .map(|(name, row)| (name.clone(), row.clone()))
            .collect::<Vec<_>>();
        rows.sort_unstable();
        rows
    }
}

impl std::ops::Deref for GenericBindings {
    type Target = HashMap<String, TypeId>;
    fn deref(&self) -> &Self::Target {
        &self.types
    }
}
impl std::ops::DerefMut for GenericBindings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.types
    }
}
impl FromIterator<(String, TypeId)> for GenericBindings {
    fn from_iter<T: IntoIterator<Item = (String, TypeId)>>(iter: T) -> Self {
        Self {
            types: iter.into_iter().collect(),
            parameters: Default::default(),
            rows: Default::default(),
        }
    }
}
impl<const N: usize> From<[(String, TypeId); N]> for GenericBindings {
    fn from(bindings: [(String, TypeId); N]) -> Self {
        bindings.into_iter().collect()
    }
}
