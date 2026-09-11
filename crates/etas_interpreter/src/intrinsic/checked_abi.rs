use std::collections::HashMap;

use etas_types::{TypeId, TypeStore, substitute_named_params_in_store};

use super::dispatch::CheckedStdIntrinsicCall;

impl CheckedStdIntrinsicCall {
    pub(crate) fn specialize(
        &self,
        store: &TypeStore,
        bindings: &HashMap<String, TypeId>,
    ) -> Result<Self, String> {
        let substitute = |ty| {
            substitute_named_params_in_store(store, ty, bindings)
                .map_err(|error| format!("checked intrinsic ABI is not materialized: {error}"))
        };
        Ok(Self {
            identity: self.identity,
            parameter_types: self
                .parameter_types
                .iter()
                .copied()
                .map(substitute)
                .collect::<Result<_, _>>()?,
            result_type: substitute(self.result_type)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intrinsic::dispatch::StdIntrinsicIdentity;
    use etas_types::{NamedTypeRef, PrimitiveType, Type, TypeInterner};

    #[test]
    fn checked_abi_specialization_requires_materialized_parameter_and_result_types() {
        let mut interner = TypeInterner::new();
        let parameter = interner.intern(Type::Named(NamedTypeRef { name: "K".into() }));
        let generic = interner.intern(Type::List(parameter));
        let concrete = interner.primitive(PrimitiveType::String);
        let bindings = HashMap::from([("K".into(), concrete)]);
        let call = CheckedStdIntrinsicCall {
            identity: StdIntrinsicIdentity {
                intrinsic: etas_std::StdIntrinsicId(etas_std::intrinsic::runtime::MEMORY_COMMIT),
                dispatch: etas_std::IntrinsicDispatch::Runtime,
            },
            parameter_types: vec![generic],
            result_type: generic,
        };
        let size = interner.store().iter().count();
        let error = call.specialize(interner.store(), &bindings).unwrap_err();
        assert!(error.contains("not materialized"), "{error}");
        assert_eq!(interner.store().iter().count(), size);
        let specialized = interner.intern(Type::List(concrete));
        let actual = call.specialize(interner.store(), &bindings).unwrap();
        assert_eq!(actual.parameter_types, [specialized]);
        assert_eq!(actual.result_type, specialized);
        assert_eq!(actual.identity, call.identity);
        assert_eq!(call.parameter_types, [generic]);
    }
}
