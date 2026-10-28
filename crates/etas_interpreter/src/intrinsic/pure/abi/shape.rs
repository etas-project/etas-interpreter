use std::collections::{HashMap, HashSet};

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
    Enum(super::enum_layout::EnumAbiLayout),
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
    nominal_identities: HashSet<TypeId>,
}

impl PureAbiProjector {
    pub fn build(
        store: &TypeStore,
        layouts: &HashMap<TypeId, etas_types::EnumLayoutFact>,
    ) -> Result<Self, String> {
        let mut interner = TypeInterner::from_store(store.clone());
        let mut representations = HashMap::new();
        let mut enums = HashMap::new();
        let mut cursor = 0_u32;
        while let Some(ty) = interner.store().get(TypeId(cursor)).cloned() {
            if matches!(ty, Type::Nominal(_) | Type::Applied { .. }) {
                if let Some(representation) = applied_representation(&mut interner, TypeId(cursor))
                    .map_err(|error| error.to_string())?
                {
                    representations.insert(TypeId(cursor), representation);
                }
            }
            let enum_instance = match &ty {
                Type::Enum(_) => Some((TypeId(cursor), Vec::new())),
                Type::Applied { constructor, args }
                    if matches!(
                        interner.store().get(TypeId(constructor.0)),
                        Some(Type::Enum(_))
                    ) =>
                {
                    Some((TypeId(constructor.0), args.clone()))
                }
                _ => None,
            };
            if let Some((owner, args)) = enum_instance {
                let Some(layout) = layouts.get(&owner) else {
                    enums.insert(
                        TypeId(cursor),
                        AbiShape::Unsupported(format!(
                            "enum {owner:?} has no checked payload layout"
                        )),
                    );
                    cursor += 1;
                    continue;
                };
                if !layout.type_params.is_empty() && args.is_empty() {
                    enums.insert(
                        TypeId(cursor),
                        AbiShape::Unsupported("unapplied generic enum constructor".into()),
                    );
                } else {
                    enums.insert(
                        TypeId(cursor),
                        AbiShape::Enum(super::enum_layout::EnumAbiLayout::build(
                            layout,
                            &args,
                            interner.store(),
                        )?),
                    );
                }
            }
            cursor += 1;
        }

        let store = interner.into_store();
        let nominal_identities = store
            .iter()
            .filter_map(|(id, ty)| {
                let nominal = match ty {
                    Type::Nominal(_) | Type::Enum(_) => true,
                    Type::Applied { constructor, .. } => {
                        matches!(
                            store.get(TypeId(constructor.0)),
                            Some(Type::Nominal(_) | Type::Enum(_))
                        )
                    }
                    _ => false,
                };
                nominal.then_some(id)
            })
            .collect();
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
                    Type::Enum(_) => enums
                        .remove(&id)
                        .ok_or_else(|| format!("enum {id:?} has no prepared ABI layout"))?,
                    Type::Applied { constructor, .. }
                        if matches!(store.get(TypeId(constructor.0)), Some(Type::Enum(_))) =>
                    {
                        enums.remove(&id).ok_or_else(|| {
                            format!("applied enum {id:?} has no prepared ABI layout")
                        })?
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
                Ok((id, shape))
            })
            .collect::<Result<_, String>>()?;
        let wrapper_walks = WrapperWalkLimits::build(&shapes);
        Ok(Self {
            shapes,
            wrapper_walks,
            nominal_identities,
        })
    }

    pub fn shape(&self, ty: TypeId) -> Option<&AbiShape> {
        self.shapes.get(&ty)
    }

    pub(super) fn wrapper_steps(&self, ty: TypeId) -> Option<usize> {
        self.wrapper_walks.get(ty)
    }

    pub(super) fn has_nominal_identity(&self, ty: TypeId) -> bool {
        self.nominal_identities.contains(&ty)
    }
}
