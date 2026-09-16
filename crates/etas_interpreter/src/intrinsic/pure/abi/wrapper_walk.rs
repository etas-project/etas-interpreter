use std::collections::HashMap;

use etas_types::TypeId;

use super::AbiShape;

#[cfg(test)]
thread_local! { static LOOKUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Exact number of wrapper edges before a terminal shape or the first repeated
/// type. This graph is immutable after prepare; queries need only a counter.
#[derive(Clone, Debug, Default)]
pub(super) struct WrapperWalkLimits(HashMap<TypeId, usize>);

impl WrapperWalkLimits {
    pub(super) fn build(shapes: &HashMap<TypeId, AbiShape>) -> Self {
        let mut limits = HashMap::new();
        let mut path = Vec::new();
        let mut active = HashMap::new();
        for root in shapes.keys().copied() {
            if limits.contains_key(&root) || successor(shapes, root).is_none() {
                continue;
            }
            let mut current = root;
            let suffix = loop {
                if let Some(limit) = limits.get(&current).copied() {
                    break limit;
                }
                if let Some(start) = active.get(&current).copied() {
                    let cycle_len = path.len() - start;
                    for ty in path.drain(start..) {
                        active.remove(&ty);
                        limits.insert(ty, cycle_len);
                    }
                    break cycle_len;
                }
                let Some(next) = successor(shapes, current) else {
                    break 0;
                };
                active.insert(current, path.len());
                path.push(current);
                current = next;
            };
            let mut length = suffix;
            for ty in path.drain(..).rev() {
                active.remove(&ty);
                length += 1;
                limits.insert(ty, length);
            }
        }
        Self(limits)
    }

    pub(super) fn get(&self, ty: TypeId) -> Option<usize> {
        self.0.get(&ty).copied()
    }
}

fn successor(shapes: &HashMap<TypeId, AbiShape>, ty: TypeId) -> Option<TypeId> {
    #[cfg(test)]
    LOOKUPS.set(LOOKUPS.get() + 1);
    match shapes.get(&ty)? {
        AbiShape::Nominal { representation } => Some(*representation),
        AbiShape::Refined { base } => Some(*base),
        AbiShape::Trust { inner, .. } => Some(*inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn prepared_limits_match_first_repeat_for_all_small_wrapper_graphs() {
        // Each node may terminate or point to any node, including itself.
        for mut edges in 0..5_usize.pow(4) {
            let mut shapes = HashMap::new();
            for id in 0..4 {
                let edge = edges % 5;
                edges /= 5;
                shapes.insert(
                    TypeId(id),
                    if edge == 4 {
                        AbiShape::Primitive(etas_types::PrimitiveType::String)
                    } else {
                        let target = TypeId(edge as u32);
                        match id % 3 {
                            0 => AbiShape::Nominal {
                                representation: target,
                            },
                            1 => AbiShape::Refined { base: target },
                            _ => AbiShape::Trust {
                                wrapper: etas_types::TrustWrapper::Trusted,
                                inner: target,
                            },
                        }
                    },
                );
            }
            let limits = WrapperWalkLimits::build(&shapes);
            for id in 0..4 {
                let root = TypeId(id);
                if successor(&shapes, root).is_none() {
                    assert_eq!(limits.get(root), None);
                    continue;
                }
                let mut visited = HashSet::new();
                let mut current = root;
                while let Some(next) = successor(&shapes, current) {
                    if !visited.insert(current) {
                        break;
                    }
                    current = next;
                }
                assert_eq!(
                    limits.get(root),
                    Some(visited.len()),
                    "{shapes:?}, root={root:?}"
                );
            }
        }
    }

    #[test]
    fn preparing_shared_wrapper_suffixes_visits_a_linear_number_of_nodes() {
        for count in [1000, 2000, 4000] {
            let mut shapes = HashMap::new();
            shapes.insert(
                TypeId(0),
                AbiShape::Primitive(etas_types::PrimitiveType::String),
            );
            for id in 1..=count {
                shapes.insert(
                    TypeId(id),
                    AbiShape::Refined {
                        base: TypeId(id - 1),
                    },
                );
            }
            for id in count + 1..=2 * count {
                shapes.insert(
                    TypeId(id),
                    AbiShape::Nominal {
                        representation: TypeId(count),
                    },
                );
            }
            LOOKUPS.set(0);
            let (limits, cost) =
                crate::testing::allocation::measure(|| WrapperWalkLimits::build(&shapes));
            assert!(
                LOOKUPS.get() <= shapes.len() * 4,
                "revisited shared suffix: n={count}"
            );
            assert!(cost.count < 128, "per-node path allocation: {cost:?}");
            assert!(
                cost.bytes < usize::try_from(count).unwrap() * 1024,
                "superlinear cache: {cost:?}"
            );
            assert_eq!(limits.0.len(), usize::try_from(2 * count).unwrap());
            assert_eq!(limits.get(TypeId(count)), Some(count as usize));
            for id in count + 1..=2 * count {
                assert_eq!(limits.get(TypeId(id)), Some(count as usize + 1));
            }
        }
    }
}
