use super::{LocalsSnapshot, SnapshotValidator};

pub(super) enum FrameValidation {
    Active(LocalsSnapshot),
    Complete(LocalsSnapshot),
}

impl SnapshotValidator<'_> {
    pub(super) fn frame(&self, frame: &LocalsSnapshot, context: &str) -> Result<(), String> {
        if frame.id == 0 {
            return Err(format!("{context} has a zero frame identity"));
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
                    FrameValidation::Complete(_) => Ok(()),
                    FrameValidation::Active(_) => {
                        Err(format!("{context} has cyclic local-frame definitions"))
                    }
                };
            }
            definitions.insert(frame.id, FrameValidation::Active(frame.clone()));
        }
        let result = self.frame_contents(frame, context);
        let mut definitions = self.frame_definitions.borrow_mut();
        if result.is_ok() {
            definitions.insert(frame.id, FrameValidation::Complete(frame.clone()));
        } else {
            definitions.remove(&frame.id);
        }
        result
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
        for (symbol, value) in frame.locals.iter() {
            self.symbol(*symbol, &format!("{context} frame local"))?;
            if self.slots.resolve(*symbol).is_none() {
                return Err(format!(
                    "{context} frame local symbol {} has no checked slot layout",
                    symbol.0
                ));
            }
            self.snapshot_value(value)?;
        }
        Ok(())
    }
}
