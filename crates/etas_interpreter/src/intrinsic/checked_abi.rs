use std::collections::HashMap;

use etas_types::{TypeId, TypeStore, substitute_named_params_in_store};

use super::dispatch::{CheckedPureIntrinsicCall, CheckedStdIntrinsicCall};

impl CheckedPureIntrinsicCall {
    pub(crate) fn specialize(
        mut self,
        store: &TypeStore,
        bindings: &crate::control::GenericBindings,
    ) -> Result<Self, String> {
        if bindings.is_empty() {
            return Ok(self);
        }
        let mut substitution = bindings.substitution(store);
        for ty in self
            .parameter_types
            .iter_mut()
            .chain(std::iter::once(&mut self.result_type))
        {
            *ty = substitution.substitute(*ty).map_err(|error| {
                format!("checked pure intrinsic ABI is not materialized: {error}")
            })?;
        }
        Ok(self)
    }
}

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
        let mut substitutions = self.substitutions.clone();
        if !bindings.is_empty() {
            let mut step = bindings
                .iter()
                .map(|(name, ty)| (name.clone(), *ty))
                .collect::<Vec<_>>();
            step.sort_by(|left, right| left.0.cmp(&right.0));
            substitutions.push(step);
        }
        Ok(Self {
            identity: self.identity,
            origin: self.origin,
            substitutions,
            parameter_types: self
                .parameter_types
                .iter()
                .copied()
                .map(substitute)
                .collect::<Result<_, _>>()?,
            result_type: substitute(self.result_type)?,
        })
    }

    pub(crate) fn instantiate_type(&self, store: &TypeStore, ty: TypeId) -> Result<TypeId, String> {
        Self::instantiate_type_with(store, ty, &self.substitutions)
    }

    pub(crate) fn instantiate_type_with(
        store: &TypeStore,
        mut ty: TypeId,
        steps: &[Vec<(String, TypeId)>],
    ) -> Result<TypeId, String> {
        for step in steps {
            let bindings = step.iter().cloned().collect::<HashMap<_, _>>();
            if step.is_empty()
                || bindings.len() != step.len()
                || step
                    .iter()
                    .any(|(name, ty)| name.is_empty() || store.get(*ty).is_none())
            {
                return Err(
                    "checked intrinsic substitution has invalid or duplicate bindings".into(),
                );
            }
            ty = substitute_named_params_in_store(store, ty, &bindings).map_err(|error| {
                format!("checked intrinsic substitution is not materialized: {error}")
            })?;
        }
        Ok(ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intrinsic::dispatch::StdIntrinsicIdentity;
    use etas_types::{NamedTypeRef, PrimitiveType, Type, TypeInterner};

    #[test]
    fn pure_abi_specialization_requires_existing_checked_result_types() {
        let mut types = TypeInterner::new();
        let param = types.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "K".into(),
        }));
        let generic = types.intern(Type::Option(param));
        let concrete = types.primitive(PrimitiveType::String);
        let call = CheckedPureIntrinsicCall {
            intrinsic: etas_std::StdIntrinsicId(etas_std::intrinsic::pure::OPTION_SOME),
            parameter_types: vec![param],
            result_type: generic,
        };
        let bindings = crate::control::GenericBindings::from([("K".into(), concrete)]);
        let size = types.store().iter().count();
        let error = call
            .clone()
            .specialize(types.store(), &bindings)
            .unwrap_err();
        assert!(error.contains("not materialized"), "{error}");
        assert_eq!(types.store().iter().count(), size);
        let result = types.intern(Type::Option(concrete));
        let specialized = call.specialize(types.store(), &bindings).unwrap();
        assert_eq!(specialized.parameter_types, [concrete]);
        assert_eq!(specialized.result_type, result);
    }

    #[test]
    fn checked_abi_specialization_requires_materialized_parameter_and_result_types() {
        let mut interner = TypeInterner::new();
        let parameter = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "K".into(),
        }));
        let generic = interner.intern(Type::List(parameter));
        let concrete = interner.primitive(PrimitiveType::String);
        let bindings = HashMap::from([("K".into(), concrete)]);
        let call = CheckedStdIntrinsicCall {
            identity: StdIntrinsicIdentity {
                intrinsic: etas_std::StdIntrinsicId(etas_std::intrinsic::runtime::MEMORY_COMMIT),
                dispatch: etas_std::IntrinsicDispatch::Runtime,
            },
            origin: etas_hir::HirExprId(7),
            substitutions: Vec::new(),
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
        assert_eq!(actual.origin, call.origin);
        assert_eq!(
            actual.instantiate_type(interner.store(), generic).unwrap(),
            specialized
        );
        assert_eq!(call.parameter_types, [generic]);
    }

    #[test]
    fn checked_intrinsic_substitutions_preserve_owner_and_callee_scope_order() {
        let mut interner = TypeInterner::new();
        let key = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "K".into(),
        }));
        let value = interner.intern(Type::Named(NamedTypeRef {
            source_parameter: None,
            name: "V".into(),
        }));
        let generic = interner.intern(Type::Tuple(vec![value, key]));
        let integer = interner.primitive(PrimitiveType::I32);
        let string = interner.primitive(PrimitiveType::String);
        let concrete = interner.intern(Type::Tuple(vec![string, integer]));
        let call = CheckedStdIntrinsicCall {
            identity: StdIntrinsicIdentity {
                intrinsic: etas_std::StdIntrinsicId(etas_std::intrinsic::runtime::MEMORY_COMMIT),
                dispatch: etas_std::IntrinsicDispatch::Runtime,
            },
            origin: etas_hir::HirExprId(7),
            substitutions: vec![],
            parameter_types: vec![generic],
            result_type: generic,
        };
        let owner = HashMap::from([("K".into(), integer), ("V".into(), string)]);
        let callee = HashMap::from([("K".into(), string), ("V".into(), integer)]);
        let specialized = call
            .specialize(interner.store(), &owner)
            .unwrap()
            .specialize(interner.store(), &callee)
            .unwrap();
        assert_eq!(specialized.parameter_types, [concrete]);
        assert_eq!(
            specialized
                .instantiate_type(interner.store(), generic)
                .unwrap(),
            concrete
        );
        assert_eq!(
            specialized
                .instantiate_type(interner.store(), value)
                .unwrap(),
            string
        );
        assert_eq!(specialized.substitutions.len(), 2);
    }
}
