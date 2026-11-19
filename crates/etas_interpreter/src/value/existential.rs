use etas_hir::HirExprId;
use etas_types::TypeId;

use crate::control::GenericBindings;

mod origin;
use origin::{ExistentialOrigin, ExistentialOriginSnapshot};

#[derive(Clone, Debug)]
pub struct ExistentialValue {
    pub(crate) pack: HirExprId,
    pub(crate) object_type: TypeId,
    pub(crate) concrete_type: TypeId,
    pub(crate) bindings: std::sync::Arc<GenericBindings>,
    pub(crate) value: super::SharedValue,
    pub(crate) origin: Option<std::sync::Arc<ExistentialOrigin>>,
}

impl ExistentialValue {
    pub(crate) fn validate(
        &self,
        checked: &etas_frontend::CheckedProject,
    ) -> Result<GenericBindings, String> {
        let header = ExistentialHeader::capture(self);
        match &self.origin {
            Some(origin) => origin.validate(&header, checked),
            None => header.validate(checked),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ExistentialEvidence<'a> {
    Runtime(&'a ExistentialValue),
    Snapshot {
        header: &'a ExistentialHeader,
        value: &'a crate::orchestration::ValueSnapshot,
        revisions: Option<&'a crate::api::code::PreparedRevisionSet>,
    },
}

impl<'a> ExistentialEvidence<'a> {
    pub(crate) fn pack(self) -> HirExprId {
        match self {
            Self::Runtime(value) => value.pack,
            Self::Snapshot { header, .. } => header.pack,
        }
    }
    pub(crate) fn object_type(self) -> TypeId {
        match self {
            Self::Runtime(value) => value.object_type,
            Self::Snapshot { header, .. } => header.object_type,
        }
    }
    pub(crate) fn source_type(self) -> TypeId {
        match self {
            Self::Runtime(value) => value.concrete_type,
            Self::Snapshot { header, .. } => header.concrete_type,
        }
    }
    pub(crate) fn bindings(
        self,
        checked: &etas_frontend::CheckedProject,
    ) -> Result<std::borrow::Cow<'a, GenericBindings>, String> {
        match self {
            Self::Runtime(value) => {
                value.validate(checked)?;
                Ok(std::borrow::Cow::Borrowed(&value.bindings))
            }
            Self::Snapshot {
                header, revisions, ..
            } => header
                .validate_snapshot(checked, revisions)
                .map(std::borrow::Cow::Owned),
        }
    }
    pub(crate) fn is_scoped(self) -> bool {
        match self {
            Self::Runtime(value) => value.origin.is_some(),
            Self::Snapshot { header, .. } => header.origin.is_some(),
        }
    }
    pub(crate) fn inner(self) -> Option<Self> {
        match self {
            Self::Runtime(value) => match value.value.as_ref() {
                super::InterpValue::Existential(value) => Some(Self::Runtime(value)),
                _ => None,
            },
            Self::Snapshot {
                value, revisions, ..
            } => match value {
                crate::orchestration::ValueSnapshot::Existential { header, value } => {
                    Some(Self::Snapshot {
                        header,
                        value,
                        revisions,
                    })
                }
                _ => None,
            },
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExistentialHeader {
    pub pack: HirExprId,
    pub object_type: TypeId,
    pub concrete_type: TypeId,
    pub type_bindings: Vec<(String, TypeId)>,
    pub type_parameter_bindings: Vec<(TypeId, TypeId)>,
    pub effect_row_bindings: Vec<(String, etas_types::EffectRowRef)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Box<ExistentialOriginSnapshot>>,
}

impl ExistentialHeader {
    pub(crate) fn validate(
        &self,
        checked: &etas_frontend::CheckedProject,
    ) -> Result<GenericBindings, String> {
        if self.origin.is_some() {
            return Err("scoped existential evidence requires its checked program origins".into());
        }
        let fact = checked
            .types
            .existential_packs
            .get(&self.pack)
            .ok_or("existential evidence has no checked packing expression")?;
        if checked.types.expr_types.get(&self.pack) != Some(&fact.object_type) {
            return Err("existential evidence disagrees with its checked result type".into());
        }
        let bindings = GenericBindings {
            types: self.type_bindings.iter().cloned().collect(),
            parameters: self.type_parameter_bindings.iter().copied().collect(),
            rows: self.effect_row_bindings.iter().cloned().collect(),
        };
        if bindings.types.len() != self.type_bindings.len()
            || bindings.parameters.len() != self.type_parameter_bindings.len()
            || bindings.rows.len() != self.effect_row_bindings.len()
        {
            return Err("existential evidence contains duplicate generic binders".into());
        }
        for ty in bindings.types.values() {
            if checked.type_store.get(*ty).is_none() {
                return Err("existential evidence references a missing checked type".into());
            }
        }
        for (parameter, argument) in &bindings.parameters {
            if !matches!(checked.type_store.get(*parameter), Some(etas_types::Type::Named(named)) if named.source_parameter.is_some())
                || checked.type_store.get(*argument).is_none()
            {
                return Err(
                    "existential evidence references an invalid checked type parameter".into(),
                );
            }
        }
        let mut substitute = bindings.substitution(&checked.type_store);
        let object_type = substitute
            .substitute(fact.object_type)
            .map_err(|error| error.to_string())?;
        let concrete_type = substitute
            .substitute(fact.source_type)
            .map_err(|error| error.to_string())?;
        if self.object_type != object_type || self.concrete_type != concrete_type {
            return Err(
                "existential evidence changes its checked producer or interface type".into(),
            );
        }
        Ok(bindings)
    }
    pub(crate) fn capture(value: &ExistentialValue) -> Self {
        Self {
            pack: value.pack,
            object_type: value.object_type,
            concrete_type: value.concrete_type,
            type_bindings: value.bindings.sorted_types(),
            type_parameter_bindings: value.bindings.sorted_parameters(),
            effect_row_bindings: value.bindings.sorted_rows(),
            origin: None,
        }
    }

    pub(crate) fn runtime(
        self,
        value: super::SharedValue,
        checked: Option<&etas_frontend::CheckedProgram>,
        revisions: Option<&crate::api::code::PreparedRevisionSet>,
    ) -> Result<ExistentialValue, String> {
        let origin = self
            .origin
            .as_ref()
            .map(|origin| {
                let checked = checked.ok_or("scoped existential restore has no checked program")?;
                let origin = origin.restore(revisions)?;
                origin.validate(&self, checked)?;
                Ok::<_, String>(std::sync::Arc::new(origin))
            })
            .transpose()?;
        Ok(ExistentialValue {
            pack: self.pack,
            object_type: self.object_type,
            concrete_type: self.concrete_type,
            bindings: GenericBindings {
                types: self.type_bindings.into_iter().collect(),
                parameters: self.type_parameter_bindings.into_iter().collect(),
                rows: self.effect_row_bindings.into_iter().collect(),
            }
            .shared(),
            value,
            origin,
        })
    }
}
