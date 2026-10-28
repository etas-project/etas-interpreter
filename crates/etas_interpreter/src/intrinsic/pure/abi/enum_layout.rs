use super::AdapterError;
use etas_types::{EnumLayoutFact, TypeId, TypeStore};
use std::collections::HashMap;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumAbiLayout {
    variants: HashMap<String, Result<Vec<TypeId>, String>>,
}

impl EnumAbiLayout {
    pub(super) fn build(
        layout: &EnumLayoutFact,
        args: &[TypeId],
        store: &TypeStore,
    ) -> Result<Self, String> {
        if layout.type_params.len() != args.len() {
            return Err("enum ABI has inconsistent generic arguments".into());
        }
        let bindings = layout
            .type_params
            .iter()
            .cloned()
            .zip(args.iter().copied())
            .collect();
        let mut variants = HashMap::new();
        for variant in &layout.variants {
            let fields = variant
                .fields
                .iter()
                .map(|ty| {
                    etas_types::substitute_named_params_in_store(store, *ty, &bindings)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>();
            if variants.insert(variant.name.clone(), fields).is_some() {
                return Err("enum ABI has duplicate checked constructors".into());
            }
        }
        Ok(Self { variants })
    }

    pub(super) fn payload(&self, name: &str, arity: usize) -> Result<&[TypeId], AdapterError> {
        let fields = self
            .variants
            .get(name)
            .ok_or_else(|| {
                AdapterError::UnsupportedValue("enum ABI received an unknown constructor".into())
            })?
            .as_ref()
            .map_err(|reason| {
                AdapterError::UnsupportedValue(format!(
                    "enum payload type is not materialized in checked facts: {reason}"
                ))
            })?;
        if fields.len() != arity {
            return Err(AdapterError::Arity {
                expected: fields.len(),
                actual: arity,
            });
        }
        Ok(fields)
    }
}
