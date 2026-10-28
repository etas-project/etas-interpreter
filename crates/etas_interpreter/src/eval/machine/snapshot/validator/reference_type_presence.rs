use super::{SnapshotValidator, TypeId};
use etas_types::Type;
use std::collections::HashSet;

#[cfg(test)]
mod tests;

impl SnapshotValidator<'_> {
    pub(super) fn type_requires_reference(&self, root: TypeId) -> Result<bool, String> {
        if let Some(found) = self.reference_type_presence.borrow().get(&root) {
            return Ok(*found);
        }
        let mut pending = vec![(root, false)];
        let mut seen = HashSet::new();
        // Inspect the finite checked declaration graph, not recursively
        // instantiate recursive ADTs. Applied arguments are checked separately
        // from the constructor's abstract parameter occurrences.
        while let Some((ty, template)) = pending.pop() {
            if !seen.insert((ty, template)) {
                continue;
            }
            let cached = if template {
                None
            } else {
                self.reference_type_presence.borrow().get(&ty).copied()
            };
            if let Some(found) = cached {
                if found {
                    self.reference_type_presence.borrow_mut().insert(root, true);
                    return Ok(true);
                }
                continue;
            }
            if self.checked.types.agent_components.contains_key(&ty) {
                self.reference_type_presence.borrow_mut().insert(root, true);
                return Ok(true);
            }
            let data = self
                .checked
                .type_store
                .get(ty)
                .ok_or("checkpoint slot references a missing checked type")?;
            // Registered opaque leaves have dedicated bounded codecs. Still
            // inspect canonical containers such as Message<T> and their T.
            if matches!(data, Type::Named(_))
                && self
                    .checked
                    .types
                    .known_std_types
                    .opaque_values
                    .contains(&ty)
            {
                continue;
            }
            match data {
                Type::Function(_) => {
                    self.reference_type_presence.borrow_mut().insert(root, true);
                    return Ok(true);
                }
                Type::Array(inner)
                | Type::List(inner)
                | Type::Set(inner)
                | Type::Slice(inner)
                | Type::Option(inner)
                | Type::Message(inner)
                | Type::Trust { inner, .. }
                | Type::Refined { base: inner, .. } => pending.push((*inner, template)),
                Type::Map { key, value }
                | Type::Result {
                    ok: key,
                    err: value,
                } => {
                    pending.push((*key, template));
                    pending.push((*value, template));
                }
                Type::Tuple(types) => pending.extend(types.iter().map(|ty| (*ty, template))),
                Type::Record(record) => {
                    pending.extend(record.fields.iter().map(|field| (field.ty, template)))
                }
                Type::Nominal(nominal) => {
                    if let Some(representation) = nominal.representation {
                        pending.push((representation, template || !nominal.params.is_empty()));
                    }
                }
                Type::Applied { constructor, args } => {
                    pending.push((TypeId(constructor.0), true));
                    pending.extend(args.iter().map(|ty| (*ty, template)));
                }
                Type::Enum(_) => {
                    let layout = self
                        .checked
                        .types
                        .enum_layouts
                        .get(&ty)
                        .ok_or("checkpoint enum slot has no checked layout")?;
                    let template = template || !layout.type_params.is_empty();
                    pending.extend(
                        layout.variants.iter().flat_map(|variant| {
                            variant.fields.iter().map(move |ty| (*ty, template))
                        }),
                    );
                }
                Type::Named(_) | Type::Var(_) if !template => {
                    return Err("checkpoint slot contains an unresolved generic value type".into());
                }
                _ => {}
            }
        }
        let mut cache = self.reference_type_presence.borrow_mut();
        // Only a completed no-reference search proves all visited concrete
        // nodes false. Abstract constructor parameters are not concrete slots.
        for (ty, template) in seen {
            if !template {
                cache.insert(ty, false);
            }
        }
        cache.insert(root, false);
        Ok(false)
    }
}
