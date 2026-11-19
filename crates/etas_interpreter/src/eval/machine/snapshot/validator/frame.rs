use super::{LocalsSnapshot, SnapshotValidator};

pub(super) enum FrameValidation {
    Active(LocalsSnapshot, usize),
    Complete(LocalsSnapshot),
}

impl SnapshotValidator<'_> {
    pub(super) fn resolve_frame(&self, frame: &LocalsSnapshot) -> Result<LocalsSnapshot, String> {
        if !frame.reference {
            return Ok(frame.clone());
        }
        frame.validate_reference()?;
        self.frame_definitions
            .borrow()
            .get(&frame.id)
            .map(|state| {
                let (FrameValidation::Active(frame, _) | FrameValidation::Complete(frame)) = state;
                frame.clone()
            })
            .ok_or_else(|| "captured frame reference has no declaring definition".into())
    }

    pub(super) fn frame(&self, frame: &LocalsSnapshot, context: &str) -> Result<(), String> {
        let depth = self.frame_depth.get();
        if self.frame_state(frame, context, depth)? {
            return Ok(());
        }
        let root = self.resolve_frame(frame)?;
        let mut pending = vec![(root.clone(), true, depth)];
        pending.extend(Self::frame_children(&root, depth).into_iter().rev());
        let result = (|| {
            while let Some((frame, complete, depth)) = pending.pop() {
                if complete {
                    self.frame_depth.set(depth);
                    self.frame_contents(&frame, context)?;
                    self.frame_definitions
                        .borrow_mut()
                        .insert(frame.id, FrameValidation::Complete(frame));
                } else if !self.frame_state(&frame, context, depth)? {
                    let frame = self.resolve_frame(&frame)?;
                    let children = Self::frame_children(&frame, depth);
                    pending.push((frame, true, depth));
                    pending.extend(children.into_iter().rev());
                }
            }
            Ok(())
        })();
        self.frame_depth.set(depth);
        if result.is_err() {
            self.frame_definitions
                .borrow_mut()
                .retain(|_, state| matches!(state, FrameValidation::Complete(_)));
        }
        result
    }

    fn frame_children(frame: &LocalsSnapshot, depth: usize) -> Vec<(LocalsSnapshot, bool, usize)> {
        let mut children = Vec::new();
        if let Some(parent) = &frame.parent {
            children.push(((**parent).clone(), false, depth));
        }
        for (symbol, value) in frame.locals.iter() {
            let mut nested = Vec::new();
            super::super::frame_dependencies::snapshot_frames(value, &mut nested);
            let depth = depth
                + usize::from(
                    frame
                        .shared_bindings
                        .iter()
                        .any(|binding| binding.symbol == *symbol),
                );
            children.extend(
                nested
                    .into_iter()
                    .map(|(frame, complete)| (frame, complete, depth)),
            );
        }
        children
    }

    fn frame_state(
        &self,
        frame: &LocalsSnapshot,
        context: &str,
        depth: usize,
    ) -> Result<bool, String> {
        if frame.id == 0 {
            return Err(format!("{context} has a zero frame identity"));
        }
        let owner = std::sync::Arc::as_ptr(self.checked) as usize;
        let resolved = if frame.reference {
            std::borrow::Cow::Owned(self.resolve_frame(frame)?)
        } else {
            std::borrow::Cow::Borrowed(frame)
        };
        let first_logical_visit = {
            let mut ownership = self.frame_owners.borrow_mut();
            if ownership
                .programs
                .insert(frame.id, owner)
                .is_some_and(|previous| previous != owner)
            {
                return Err(format!(
                    "{context} local frame is shared across different checked programs"
                ));
            }
            ownership.observe(&resolved, owner, self.task_identity)?
        };
        let mut definitions = self.frame_definitions.borrow_mut();
        if frame.reference {
            frame.validate_reference()?;
            return match definitions.get(&frame.id) {
                Some(FrameValidation::Complete(_)) if first_logical_visit => {
                    definitions.insert(
                        frame.id,
                        FrameValidation::Active(resolved.into_owned(), depth),
                    );
                    Ok(false)
                }
                Some(FrameValidation::Complete(_)) => Ok(true),
                Some(FrameValidation::Active(_, previous)) if depth > *previous => Ok(true),
                Some(FrameValidation::Active(_, _)) => Err(format!(
                    "{context} has a frame cycle without a shared binding"
                )),
                None => Err(format!(
                    "{context} has an unresolved captured frame reference"
                )),
            };
        }
        if let Some(state) = definitions.get(&frame.id) {
            let (FrameValidation::Active(existing, _) | FrameValidation::Complete(existing)) =
                state;
            if existing != frame {
                return Err(format!("{context} has conflicting local-frame definitions"));
            }
            return match state {
                FrameValidation::Complete(existing)
                    if !first_logical_visit
                        && std::rc::Rc::ptr_eq(&existing.locals, &frame.locals)
                        && match (&existing.parent, &frame.parent) {
                            (None, None) => true,
                            (Some(left), Some(right)) => std::rc::Rc::ptr_eq(left, right),
                            _ => false,
                        } =>
                {
                    Ok(true)
                }
                FrameValidation::Complete(_) => {
                    // Re-rooting a cyclic graph can expand an earlier reference
                    // into a full occurrence. Check that occurrence's children
                    // and contracts before caching its equivalent definition.
                    definitions.insert(frame.id, FrameValidation::Active(frame.clone(), depth));
                    Ok(false)
                }
                FrameValidation::Active(_, _) => {
                    Err(format!("{context} has cyclic local-frame definitions"))
                }
            };
        }
        definitions.insert(frame.id, FrameValidation::Active(frame.clone(), depth));
        Ok(false)
    }

    fn frame_contents(&self, frame: &LocalsSnapshot, context: &str) -> Result<(), String> {
        let mut bindings = std::collections::BTreeSet::new();
        let mut identities = std::collections::BTreeSet::new();
        for binding in &frame.shared_bindings {
            if binding.id == 0
                || binding.owner == 0
                || !bindings.insert(binding.symbol)
                || !identities.insert(binding.id)
                || !frame
                    .locals
                    .iter()
                    .any(|(symbol, _)| *symbol == binding.symbol)
                || !matches!(
                    self.checked.types.symbol_types.get(&binding.symbol),
                    Some(etas_types::SymbolTypeFact::Local { mutable: true, .. })
                )
            {
                return Err(format!(
                    "{context} has invalid captured mutable binding metadata"
                ));
            }
        }
        self.type_parameter_bindings(&frame.type_parameter_bindings, context)?;
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
        let mut activations = std::collections::BTreeMap::new();
        for saved in &frame.handler_activations {
            let revisions = self
                .revisions
                .ok_or("Handler frame requires freshly prepared code")?;
            let activation = saved.restore(self.checked, revisions, &bindings)?;
            if activations.insert(saved.arm, activation).is_some() {
                return Err("frame duplicates a Handler activation".into());
            }
        }
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
        let mut handler_bindings = std::collections::HashMap::new();
        for (binder, value) in frame.locals.iter() {
            let crate::orchestration::ValueSnapshot::TaskRuntime(
                crate::orchestration::TaskValueSnapshot::Continuation { continuation, .. },
            ) = value
            else {
                continue;
            };
            let Some(binding) = self.checked.types.continuation_bindings.get(binder) else {
                continue;
            };
            if !matches!(
                self.checked.type_store.get(binding.action_output),
                Some(etas_types::Type::Named(_) | etas_types::Type::Var(_))
            ) {
                continue;
            }
            let output = self
                .tasks
                .and_then(|tasks| {
                    tasks
                        .continuations
                        .entries
                        .get(&crate::driver::scheduler::ContinuationId(*continuation))
                })
                .ok_or("Handler continuation has no scheduler signature")?
                .output;
            let scope = self
                .checked
                .hir
                .scopes
                .get(binding.owner_scope)
                .ok_or("Handler has no checked binding scope")?;
            for symbol in &scope.symbols {
                let local = handler_bindings
                    .entry(*symbol)
                    .or_insert_with(|| bindings.clone());
                local.parameters.insert(binding.action_output, output);
            }
        }
        let depth = self.frame_depth.get();
        for (symbol, value) in frame.locals.iter() {
            self.frame_depth.set(
                depth
                    + usize::from(
                        frame
                            .shared_bindings
                            .iter()
                            .any(|binding| binding.symbol == *symbol),
                    ),
            );
            self.symbol(*symbol, &format!("{context} frame local"))?;
            if self.slots.resolve(*symbol).is_none() {
                return Err(format!(
                    "{context} frame local symbol {} has no checked slot layout",
                    symbol.0
                ));
            }
            self.snapshot_value_in(value, super::call_target::ReferenceScope::CheckedSlot)?;
            let activation = self
                .checked
                .symbols
                .get(*symbol)
                .and_then(|symbol| match &symbol.def {
                    etas_hir::SymbolDef::PatternBinding { pattern, .. } => {
                        crate::control::handler_activation::enclosing_handler(
                            self.checked,
                            etas_hir::HirOwner::Pattern(*pattern),
                        )
                    }
                    _ => None,
                })
                .and_then(|arm| activations.get(&arm));
            self.validate_local_references(
                *symbol,
                value,
                activation
                    .map(|activation| &activation.bindings)
                    .or_else(|| handler_bindings.get(symbol))
                    .unwrap_or(&bindings),
            )
            .map_err(|reason| {
                let local = self
                    .checked
                    .symbols
                    .get(*symbol)
                    .expect("validated local symbol");
                format!("{context} local `{}`: {reason}", local.name)
            })?;
        }
        self.frame_depth.set(depth);
        Ok(())
    }

    pub(super) fn type_parameter_bindings(
        &self,
        parameters: &[(etas_types::TypeId, etas_types::TypeId)],
        context: &str,
    ) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for (parameter, argument) in parameters {
            if !seen.insert(*parameter)
                || !crate::control::GenericBindings::is_checked_parameter(self.checked, *parameter)
            {
                return Err(format!(
                    "{context} has an invalid checked type-parameter binding"
                ));
            }
            self.type_id(*argument, context)?;
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

    pub(super) fn declaring_scope(
        &self,
        block: etas_hir::HirBlockId,
        frame: &LocalsSnapshot,
    ) -> Result<(), String> {
        let frame = self.resolve_frame(frame)?;
        let program = std::sync::Arc::as_ptr(self.checked) as usize;
        for binding in &frame.shared_bindings {
            if binding.owner != frame.id {
                continue;
            }
            let declaration =
                self.checked
                    .symbols
                    .get(binding.symbol)
                    .and_then(|symbol| match symbol.def {
                        etas_hir::SymbolDef::Local { binding, .. } => {
                            self.checked.hir.index().stmt_block.get(&binding).copied()
                        }
                        _ => None,
                    });
            if declaration == Some(block) {
                self.frame_owners
                    .borrow_mut()
                    .declare(*binding, program, self.task_identity)?;
            }
        }
        Ok(())
    }

    pub(super) fn transfer_value(
        &self,
        value: &crate::orchestration::ValueSnapshot,
        destination: crate::driver::scheduler::TaskIdentity,
    ) -> Result<(), String> {
        let previous = self
            .frame_owners
            .borrow_mut()
            .affinity_override
            .replace(destination);
        let previous_targets = std::mem::take(&mut *self.validated_call_targets.borrow_mut());
        let result = self.snapshot_value_in(value, super::call_target::ReferenceScope::CheckedSlot);
        *self.validated_call_targets.borrow_mut() = previous_targets;
        self.frame_owners.borrow_mut().affinity_override = previous;
        result
    }

    pub(super) fn validate_binding_owners(&self) -> Result<(), String> {
        let ownership = self.frame_owners.borrow();
        if ownership.deferred {
            return Ok(());
        }
        ownership.validate(&self.frame_definitions.borrow())
    }
}
