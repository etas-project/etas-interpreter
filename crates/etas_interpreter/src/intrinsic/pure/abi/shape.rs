use std::collections::HashMap;

use etas_types::{
    PrimitiveType, TrustWrapper, Type, TypeId, TypeInterner, TypeStore, applied_representation,
};

use super::record::RecordAbiLayout;
use super::wrapper_walk::WrapperWalkLimits;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiShape {
    Primitive(PrimitiveType),
    Array(TypeId),
    List(TypeId),
    Map {
        key: TypeId,
        value: TypeId,
    },
    Set(TypeId),
    Range(TypeId),
    Slice(TypeId),
    Option(TypeId),
    Result {
        ok: TypeId,
        err: TypeId,
    },
    Record(RecordAbiLayout),
    Tuple(Vec<TypeId>),
    Enum,
    Nominal {
        representation: TypeId,
    },
    Refined {
        base: TypeId,
    },
    Trust {
        wrapper: TrustWrapper,
        inner: TypeId,
    },
    Unsupported(String),
}

#[derive(Clone, Debug, Default)]
pub struct PureAbiProjector {
    shapes: HashMap<TypeId, AbiShape>,
    wrapper_walks: WrapperWalkLimits,
}

impl PureAbiProjector {
    pub fn build(store: &TypeStore) -> Result<Self, etas_types::TypeSubstitutionError> {
        let mut interner = TypeInterner::from_store(store.clone());
        let mut representations = HashMap::new();
        let mut cursor = 0_u32;
        while let Some(ty) = interner.store().get(TypeId(cursor)).cloned() {
            if matches!(ty, Type::Nominal(_) | Type::Applied { .. }) {
                if let Some(representation) = applied_representation(&mut interner, TypeId(cursor))?
                {
                    representations.insert(TypeId(cursor), representation);
                }
            }
            cursor += 1;
        }

        let store = interner.into_store();
        let shapes = store
            .iter()
            .map(|(id, ty)| {
                let shape = match ty {
                    Type::Primitive(primitive) => AbiShape::Primitive(*primitive),
                    Type::Array(inner) => AbiShape::Array(*inner),
                    Type::List(inner) => AbiShape::List(*inner),
                    Type::Map { key, value } => AbiShape::Map {
                        key: *key,
                        value: *value,
                    },
                    Type::Set(inner) => AbiShape::Set(*inner),
                    Type::Range { index } => AbiShape::Range(*index),
                    Type::Slice(inner) => AbiShape::Slice(*inner),
                    Type::Option(inner) => AbiShape::Option(*inner),
                    Type::Result { ok, err } => AbiShape::Result { ok: *ok, err: *err },
                    Type::Record(record) => match RecordAbiLayout::build(&record.fields) {
                        Ok(layout) => AbiShape::Record(layout),
                        Err(message) => AbiShape::Unsupported(message),
                    },
                    Type::Tuple(elements) => AbiShape::Tuple(elements.clone()),
                    Type::Enum(_) => AbiShape::Enum,
                    Type::Applied { constructor, .. }
                        if matches!(store.get(TypeId(constructor.0)), Some(Type::Enum(_))) =>
                    {
                        AbiShape::Enum
                    }
                    Type::Nominal(_) | Type::Applied { .. } => representations
                        .get(&id)
                        .copied()
                        .map(|representation| AbiShape::Nominal { representation })
                        .unwrap_or_else(|| {
                            AbiShape::Unsupported(
                                "nominal type has no checked representation".to_owned(),
                            )
                        }),
                    Type::Refined { base, .. } => AbiShape::Refined { base: *base },
                    Type::Trust { wrapper, inner } => AbiShape::Trust {
                        wrapper: *wrapper,
                        inner: *inner,
                    },
                    unsupported => AbiShape::Unsupported(format!("{unsupported:?}")),
                };
                (id, shape)
            })
            .collect();
        let wrapper_walks = WrapperWalkLimits::build(&shapes);
        Ok(Self {
            shapes,
            wrapper_walks,
        })
    }

    pub fn shape(&self, ty: TypeId) -> Option<&AbiShape> {
        self.shapes.get(&ty)
    }

    pub(super) fn wrapper_steps(&self, ty: TypeId) -> Option<usize> {
        self.wrapper_walks.get(ty)
    }
}
