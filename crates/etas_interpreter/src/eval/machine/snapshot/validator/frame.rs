use super::{LocalsSnapshot, SnapshotValidator};

pub(super) enum FrameValidation {
    Active(LocalsSnapshot),
    Complete(LocalsSnapshot),
}

impl SnapshotValidator<'_> {
    pub(super) fn frame(&self, frame: &LocalsSnapshot, context: &str) -> Result<(), String> {
        if self.frame_state(frame, context)? {
            return Ok(());
        }
        let mut pending = vec![(frame.clone(), true)];
        let mut children = Vec::new();
        for (_, value) in frame.locals.iter() {
            super::super::frame_dependencies::snapshot_frames(value, &mut children);
        }
        pending.extend(children.into_iter().rev());
        let result = (|| {
            while let Some((frame, complete)) = pending.pop() {
                if complete {
                    self.frame_contents(&frame, context)?;
                    self.frame_definitions
                        .borrow_mut()
                        .insert(frame.id, FrameValidation::Complete(frame));
                } else if !self.frame_state(&frame, context)? {
                    let mut children = Vec::new();
                    for (_, value) in frame.locals.iter() {
                        super::super::frame_dependencies::snapshot_frames(value, &mut children);
                    }
                    pending.push((frame, true));
                    pending.extend(children.into_iter().rev());
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.frame_definitions
                .borrow_mut()
                .retain(|_, state| matches!(state, FrameValidation::Complete(_)));
        }
        result
    }

    // Returns true only for a definition whose complete dependency graph passed.
    fn frame_state(&self, frame: &LocalsSnapshot, context: &str) -> Result<bool, String> {
        if frame.id == 0 {
            return Err(format!("{context} has a zero frame identity"));
        }
        let owner = std::sync::Arc::as_ptr(self.checked) as usize;
        if let Some(previous) = self.frame_owners.borrow_mut().insert(frame.id, owner)
            && previous != owner
        {
            return Err(format!(
                "{context} local frame is shared across different checked programs"
            ));
        }
        {
            let mut definitions = self.frame_definitions.borrow_mut();
            if let Some(state) = definitions.get(&frame.id) {
                let (FrameValidation::Active(existing) | FrameValidation::Complete(existing)) =
                    state;
                if existing != frame {
                    return Err(format!("{context} has conflicting local-frame definitions"));
                }
                return match state {
                    FrameValidation::Complete(_) => Ok(true),
                    FrameValidation::Active(_) => {
                        Err(format!("{context} has cyclic local-frame definitions"))
                    }
                };
            }
            definitions.insert(frame.id, FrameValidation::Active(frame.clone()));
        }
        Ok(false)
    }

    fn frame_contents(&self, frame: &LocalsSnapshot, context: &str) -> Result<(), String> {
        let mut type_params = std::collections::BTreeSet::new();
        for (name, ty) in &frame.type_bindings {
            if name.is_empty() || !type_params.insert(name) {
                return Err(format!(
                    "{context} frame contains an invalid or duplicate type parameter binding"
                ));
            }
            self.type_id(*ty, &format!("{context} frame type binding"))?;
        }
        let bindings = frame.generic_bindings();
        if frame
            .effect_row_bindings
            .iter()
            .any(|(name, _)| type_params.contains(name))
        {
            return Err(format!(
                "{context} frame binds one generic parameter as both a type and an effect row"
            ));
        }
        self.effect_row_bindings(&frame.effect_row_bindings, context)?;
        for (symbol, value) in frame.locals.iter() {
            self.symbol(*symbol, &format!("{context} frame local"))?;
            if self.slots.resolve(*symbol).is_none() {
                return Err(format!(
                    "{context} frame local symbol {} has no checked slot layout",
                    symbol.0
                ));
            }
            self.snapshot_value_in(value, super::call_target::ReferenceScope::CheckedSlot)?;
            self.validate_local_references(*symbol, value, &bindings)?;
        }
        Ok(())
    }

    pub(super) fn effect_row_bindings(
        &self,
        rows: &[(String, etas_types::EffectRowRef)],
        context: &str,
    ) -> Result<(), String> {
        let mut names = std::collections::HashSet::new();
        for (name, row) in rows {
            if name.is_empty() || !names.insert(name) {
                return Err(format!(
                    "{context} contains an invalid or duplicate effect-row binding"
                ));
            }
            if row.tail.is_some() {
                return Err(format!(
                    "{context} contains an unresolved effect-row binding"
                ));
            }
            for effect in &row.effects {
                for argument in &effect.args {
                    if let etas_types::EffectArgRef::Type(ty) = argument {
                        self.type_id(*ty, context)?;
                    }
                }
            }
            etas_effects::validate::resolve_checked_type_row(
                &self.checked.type_store,
                &self.checked.effect_registry,
                row,
            )?;
        }
        Ok(())
    }
}
