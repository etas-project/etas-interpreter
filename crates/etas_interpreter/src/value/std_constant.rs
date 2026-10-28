use etas_std::StdValueRole;
use etas_types::{KnownStdTypes, TypeId};

/// A standard data constant, not an authority grant or executable target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdConstantValue {
    role: StdValueRole,
    ty: TypeId,
}

impl StdConstantValue {
    pub fn role(self) -> StdValueRole {
        self.role
    }
    pub fn ty(self) -> TypeId {
        self.ty
    }

    pub(crate) fn checked(
        role: StdValueRole,
        ty: TypeId,
        known: &KnownStdTypes,
    ) -> Result<Self, String> {
        let value = Self { role, ty };
        value.validate(known)?;
        Ok(value)
    }

    pub(crate) fn from_parts(role: StdValueRole, ty: TypeId) -> Self {
        Self { role, ty }
    }

    pub(crate) fn validate(self, known: &KnownStdTypes) -> Result<(), String> {
        if known.runtime_value(self.role) != Some(self.ty) {
            return Err("standard constant has no matching checked declaration identity".into());
        }
        Ok(())
    }
}
