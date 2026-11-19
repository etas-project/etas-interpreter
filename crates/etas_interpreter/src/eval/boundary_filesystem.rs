use super::*;
use etas_host::{HostError, HostErrorCode};

#[derive(Clone, Copy)]
enum SourceFsAction {
    Read,
    Write,
    List,
    Stat,
    AtomicReplace,
}

impl EvalContext<'_> {
    /// Execute raw source Fs actions after language handlers and the
    /// checked residual policy. Source owns path algorithms and Result policy.
    pub(crate) fn source_filesystem_action(
        &mut self,
        perform: &PendingPerform,
    ) -> Option<ControlSignal> {
        let proof = perform.checked_action.as_ref()?;
        let action = match &proof.effect {
            etas_effects::Effect::Action(action) => action,
            etas_effects::Effect::AppliedAction(instance) => &instance.action,
            _ => return None,
        };
        let tag = etas_std::StdSourceRole::EffectFs
            .effect_execution()
            .unwrap()
            .0;
        if action.tag.0 != tag {
            return None;
        }
        let operation = [
            ("read", SourceFsAction::Read),
            ("write", SourceFsAction::Write),
            ("list", SourceFsAction::List),
            ("stat", SourceFsAction::Stat),
            ("atomic_replace", SourceFsAction::AtomicReplace),
        ]
        .into_iter()
        .find_map(|(name, operation)| {
            let symbol = self
                .checked
                .std_registry
                .lookup_qualified(&["std", "effects", "actions", "Fs", name])?;
            let etas_std::StdDecl::EffectAction(declaration) = &symbol.decl else {
                return None;
            };
            (declaration.stable_id == Some(action.action.0)).then_some(operation)
        })?;
        let signal = self.start_source_filesystem_action(perform, operation);
        Some(self.continue_chain_signal(signal, perform.continuation.clone()))
    }

    fn start_source_filesystem_action(
        &mut self,
        perform: &PendingPerform,
        action: SourceFsAction,
    ) -> ControlSignal {
        let proof = perform.checked_action.as_ref().unwrap();
        let fail = |reason: &str| ControlSignal::missing_checked_fact(reason, perform.span);
        let etas_effects::Effect::AppliedAction(instance) = &proof.effect else {
            return fail("Fs action requires its checked region selector");
        };
        if !std::sync::Arc::ptr_eq(&proof.program, self.checked)
            || !self
                .plan
                .action_mediation
                .has_default_action(&instance.action)
        {
            return fail("Fs action is absent from the checked default mediation plan");
        }
        let (Some(path_type), Some(path)) = (proof.parameters.first(), perform.args.first()) else {
            return fail("source Fs action requires its checked path payload");
        };
        let path = match self.source_workspace_path(path, *path_type, perform.span) {
            Ok(path) => path,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        // The checked selector supplies authority identity. The source record
        // contains only a relative locator, never a host root or a capability.
        let [etas_types::EffectArgRef::Type(region)] = instance.args.as_slice() else {
            return fail("Fs action has no single checked region selector");
        };
        let selector =
            match self.canonical_region_identity(*region, &Default::default(), perform.span) {
                Ok(region) => region,
                Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            };
        if selector != path.region.as_str() {
            return fail("Fs action selector and path region differ");
        }
        let prepared = self.source_filesystem_operation(perform, action, path);
        let (operation, decode) = match prepared {
            Ok(prepared) => prepared,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Filesystem(etas_host::FilesystemRequest {
                id: self.next_host_request_id(),
                operation,
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode,
            span: perform.span,
            continuation: Continuation::BlockValue,
        })
    }

    fn source_filesystem_operation(
        &self,
        perform: &PendingPerform,
        action: SourceFsAction,
        path: etas_host::WorkspacePathRef,
    ) -> Result<(etas_host::FilesystemOperation, HostBoundaryDecode), crate::control::ExecutionFault>
    {
        use etas_host::FilesystemOperation;
        use etas_types::{PrimitiveType, Type};
        let proof = perform.checked_action.as_ref().unwrap();
        let fail = |reason: &str| {
            crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                perform.span,
                reason,
            )
        };
        match (action, perform.args.as_slice(), proof.parameters.as_slice()) {
            (SourceFsAction::Read, [_, maximum], [_, maximum_type]) => {
                let Some((ok, err)) = etas_types::value_representation::result_parameters(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types,
                    proof.output,
                ) else {
                    return Err(fail("source Fs.read must return its checked Result type"));
                };
                if self.checked.type_store.get(ok) != Some(&Type::Primitive(PrimitiveType::Bytes))
                    || Some(err)
                        != self.filesystem_support_type(
                            "ReadLimitError",
                            self.known_std_types.file_read_limit_error,
                        )
                {
                    return Err(fail(
                        "source Fs.read result differs from its minimum host ABI",
                    ));
                }
                if etas_types::value_representation::option_parameter(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types,
                    *maximum_type,
                )
                .and_then(|ty| self.checked.type_store.get(ty))
                    != Some(&Type::Primitive(PrimitiveType::USize))
                {
                    return Err(fail("Fs.read byte limit must use checked Option<usize>"));
                }
                let max_bytes = match self.filesystem_option(maximum, *maximum_type) {
                    Some(None) => None,
                    Some(Some(value)) => {
                        Some(self.usize_argument(value, "Fs.read byte limit", perform.span)?)
                    }
                    _ => {
                        return Err(fail(
                            "Fs.read byte limit has no checked Option representation",
                        ));
                    }
                };
                Ok((
                    match max_bytes {
                        Some(max_bytes) => FilesystemOperation::ReadLimited { path, max_bytes },
                        None => FilesystemOperation::Read { path },
                    },
                    HostBoundaryDecode::SourceFileRead {
                        result_type: proof.output,
                        max_bytes,
                    },
                ))
            }
            (SourceFsAction::Write | SourceFsAction::AtomicReplace, [_, contents], [_, _]) => {
                let InterpValue::Bytes(contents) = contents else {
                    return Err(fail("source Fs write payload must be checked bytes"));
                };
                if self.checked.type_store.get(proof.output)
                    != Some(&Type::Primitive(PrimitiveType::Unit))
                {
                    return Err(fail("source Fs write action must return unit"));
                }
                let contents = contents.to_vec();
                let operation = match action {
                    SourceFsAction::Write => FilesystemOperation::Write {
                        path,
                        contents,
                        create_dirs: false,
                    },
                    SourceFsAction::AtomicReplace => {
                        FilesystemOperation::AtomicReplace { path, contents }
                    }
                    _ => unreachable!(),
                };
                Ok((operation, HostBoundaryDecode::Unit))
            }
            (SourceFsAction::List, [_, limits], [path_type, limits_type]) => {
                let Some((ok, err)) = etas_types::value_representation::result_parameters(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types,
                    proof.output,
                ) else {
                    return Err(fail("source Fs.list must return its checked Result type"));
                };
                if etas_types::value_representation::sequence_element_in_store(
                    &self.checked.type_store,
                    ok,
                )
                .map_err(|_| fail("Fs.list has no checked list representation"))?
                    != Some(*path_type)
                    || Some(err)
                        != self.filesystem_support_type(
                            "ListLimitError",
                            self.checked
                                .types
                                .known_std_types
                                .directory_list_limit_error,
                        )
                {
                    return Err(fail(
                        "source Fs.list result differs from its minimum host ABI",
                    ));
                }
                let tuple = etas_types::value_representation::option_parameter(
                    &self.checked.type_store,
                    &self.checked.types.known_std_types,
                    *limits_type,
                )
                .and_then(|ty| self.checked.type_store.get(ty));
                if !matches!(tuple, Some(Type::Tuple(fields)) if fields.len() == 2 && fields.iter().all(|ty| self.checked.type_store.get(*ty) == Some(&Type::Primitive(PrimitiveType::USize))))
                {
                    return Err(fail(
                        "Fs.list limits must use checked Option<(usize, usize)>",
                    ));
                }
                let limits = match self.filesystem_option(limits, *limits_type) {
                    Some(None) => None,
                    Some(Some(value)) => {
                        let InterpValue::Tuple(values) = value else {
                            return Err(fail(
                                "Fs.list limits have no checked tuple representation",
                            ));
                        };
                        let [max_entries, max_bytes] = values.as_slice() else {
                            return Err(fail("Fs.list requires entry and byte limits"));
                        };
                        Some((
                            self.usize_argument(max_entries, "Fs.list entry limit", perform.span)?,
                            self.usize_argument(max_bytes, "Fs.list byte limit", perform.span)?,
                        ))
                    }
                    _ => return Err(fail("Fs.list limits have no checked Option representation")),
                };
                Ok((
                    match limits {
                        Some((max_entries, max_bytes)) => FilesystemOperation::ReadDirLimited {
                            path,
                            max_entries,
                            max_bytes,
                        },
                        None => FilesystemOperation::ReadDir { path },
                    },
                    HostBoundaryDecode::SourceDirectoryList {
                        result_type: proof.output,
                        limits,
                    },
                ))
            }
            (SourceFsAction::Stat, [_], [_]) => Ok((
                FilesystemOperation::Stat { path },
                HostBoundaryDecode::SourceFilesystemStat {
                    result_type: proof.output,
                },
            )),
            _ => Err(fail(
                "source Fs action payload differs from its minimum host ABI",
            )),
        }
    }

    fn filesystem_support_type(
        &self,
        name: &str,
        legacy: Option<etas_types::TypeId>,
    ) -> Option<etas_types::TypeId> {
        if !self.checked.hir.source_standard_types {
            return legacy;
        }
        self.checked.hir.items.iter().find_map(|(_, item)| {
            let symbol = match item {
                HirItem::Type(item) => item.symbol,
                HirItem::Enum(item) => item.symbol,
                _ => return None,
            };
            let declaration = self.checked.symbols.get(symbol)?;
            let module = self
                .checked
                .hir
                .modules_arena
                .get(declaration.defining_module)?;
            if declaration.name.rsplit('.').next()? != name
                || !module
                    .name
                    .as_ref()?
                    .segments
                    .iter()
                    .map(|part| part.name.as_str())
                    .eq(["std", "fs"])
            {
                return None;
            }
            match self.checked.types.symbol_types.get(&symbol)? {
                etas_types::SymbolTypeFact::Type { constructor }
                | etas_types::SymbolTypeFact::NominalType { constructor, .. } => {
                    Some(etas_types::TypeId(constructor.0))
                }
                _ => None,
            }
        })
    }

    fn filesystem_option<'a>(
        &self,
        value: &'a InterpValue,
        ty: etas_types::TypeId,
    ) -> Option<Option<&'a InterpValue>> {
        etas_types::value_representation::option_parameter(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            ty,
        )?;
        match (self.checked.type_store.get(ty)?, value) {
            (etas_types::Type::Option(_), InterpValue::OptionNone) => Some(None),
            (etas_types::Type::Option(_), InterpValue::OptionSome(value)) => Some(Some(value)),
            (etas_types::Type::Applied { .. }, InterpValue::Nominal { ty: actual, value })
                if *actual == ty =>
            {
                match value.as_ref() {
                    InterpValue::Variant { name, fields }
                        if name == "None" && fields.is_empty() =>
                    {
                        Some(None)
                    }
                    InterpValue::Variant { name, fields } if name == "Some" => {
                        match fields.as_slice() {
                            [value] => Some(Some(value)),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn source_workspace_path(
        &self,
        value: &InterpValue,
        ty: etas_types::TypeId,
        span: Span,
    ) -> Result<etas_host::WorkspacePathRef, crate::control::ExecutionFault> {
        let region = self.workspace_path_region(ty, &Default::default(), span)?;
        let InterpValue::Record(record) = super::std_call::nominal_representation_ref(value) else {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source path has no checked relative-locator record",
            ));
        };
        let Some(InterpValue::String(relative)) = record.get("relative") else {
            return Err(crate::control::ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                "source path has no checked relative string",
            ));
        };
        etas_host::WorkspaceRegionId::new(region)
            .and_then(|region| etas_host::WorkspacePathRef::new(region, relative.to_string()))
            .map_err(|error| {
                crate::control::ExecutionFault::new(
                    AnalysisDiagnosticCode::InvalidArguments,
                    span,
                    error.message,
                )
            })
    }

    pub(crate) fn decode_source_file_read(
        &self,
        result_type: etas_types::TypeId,
        maximum: Option<usize>,
        entry: etas_host::FilesystemEntry,
    ) -> Result<InterpValue, String> {
        use etas_host::{BoundedFileRead, FilesystemEntry, HostValue};
        let result = match (maximum, entry) {
            (None, FilesystemEntry::Bytes(bytes)) => ("Ok", HostValue::Bytes(bytes)),
            (Some(maximum), FilesystemEntry::LimitedRead(BoundedFileRead::Complete(bytes)))
                if bytes.len() <= maximum =>
            {
                ("Ok", HostValue::Bytes(bytes))
            }
            (
                Some(maximum),
                FilesystemEntry::LimitedRead(BoundedFileRead::LimitExceeded { max_bytes }),
            ) if max_bytes == maximum => (
                "Err",
                HostValue::Variant {
                    name: "LimitExceeded".into(),
                    fields: vec![HostValue::UInt(maximum as u128)],
                },
            ),
            _ => {
                return Err("Fs.read host response differs from its checked limit contract".into());
            }
        };
        super::host_to_checked_interp_value(
            HostValue::Variant {
                name: result.0.into(),
                fields: vec![result.1],
            },
            result_type,
            self.checked,
            &self.storage_limits,
        )
    }

    pub(crate) fn decode_source_directory_list(
        &self,
        result_type: etas_types::TypeId,
        limits: Option<(usize, usize)>,
        parent: &etas_host::WorkspacePathRef,
        entry: etas_host::FilesystemEntry,
    ) -> Result<InterpValue, String> {
        use etas_host::{BoundedDirectoryList, FilesystemEntry, HostValue};
        let entries = match (limits, entry) {
            (None, FilesystemEntry::Entries(entries))
            | (Some(_), FilesystemEntry::LimitedEntries(BoundedDirectoryList::Complete(entries))) => {
                entries
            }
            (
                Some((expected_entries, expected_bytes)),
                FilesystemEntry::LimitedEntries(BoundedDirectoryList::LimitExceeded {
                    max_entries,
                    max_bytes,
                }),
            ) if (max_entries, max_bytes) == (expected_entries, expected_bytes) => {
                return super::host_to_checked_interp_value(
                    HostValue::Variant {
                        name: "Err".into(),
                        fields: vec![HostValue::Variant {
                            name: "LimitExceeded".into(),
                            fields: vec![
                                HostValue::UInt(max_entries as u128),
                                HostValue::UInt(max_bytes as u128),
                            ],
                        }],
                    },
                    result_type,
                    self.checked,
                    &self.storage_limits,
                );
            }
            _ => {
                return Err("Fs.list host response differs from its checked limit contract".into());
            }
        };
        let expected_parent = if parent.relative.as_os_str() == "." {
            std::path::Path::new("")
        } else {
            &parent.relative
        };
        let count = entries.len();
        let mut bytes = 0usize;
        let mut names = std::collections::BTreeSet::new();
        let mut paths = Vec::with_capacity(count);
        for entry in entries {
            let normalized =
                etas_host::WorkspacePathRef::new(entry.region.clone(), &entry.relative)
                    .map_err(|error| error.message)?;
            if entry != normalized
                || entry.region != parent.region
                || entry.relative.parent() != Some(expected_parent)
                || !names.insert(entry.relative.clone())
            {
                return Err(
                    "Fs.list host entry has inconsistent region, parent or identity".into(),
                );
            }
            let relative = entry.relative_name().map_err(|error| error.message)?;
            bytes = bytes
                .checked_add(relative.len())
                .ok_or("Fs.list response size overflow")?;
            paths.push(HostValue::Record(vec![(
                "relative".into(),
                HostValue::String(relative.into()),
            )]));
        }
        if limits.is_some_and(|(max_entries, max_bytes)| count > max_entries || bytes > max_bytes) {
            return Err("Fs.list host response exceeded requested limits".into());
        }
        super::host_to_checked_interp_value(
            HostValue::Variant {
                name: "Ok".into(),
                fields: vec![HostValue::List(paths)],
            },
            result_type,
            self.checked,
            &self.storage_limits,
        )
    }

    pub(crate) fn decode_source_filesystem_stat(
        &self,
        result_type: etas_types::TypeId,
        entry: etas_host::FilesystemEntry,
    ) -> Result<InterpValue, String> {
        use etas_host::{FilesystemEntry, HostValue};
        let FilesystemEntry::Stat(stat) = entry else {
            return Err("Fs.stat host response has no checked stat record".into());
        };
        super::host_to_checked_interp_value(
            HostValue::Record(vec![
                ("is_file".into(), HostValue::Bool(stat.is_file)),
                ("is_dir".into(), HostValue::Bool(stat.is_dir)),
                ("len".into(), HostValue::UInt(stat.len as u128)),
            ]),
            result_type,
            self.checked,
            &self.storage_limits,
        )
    }

    pub(crate) fn filesystem_io_failure_signal(
        &mut self,
        boundary: PendingHostBoundary,
        error: HostError,
    ) -> ControlSignal {
        if let Some(signal) = self.cancellation_signal(boundary.span) {
            return signal;
        }
        if error.code != HostErrorCode::IoFailure {
            return ControlSignal::missing_checked_fact(
                "filesystem IOError projection requires a tagged I/O failure",
                boundary.span,
            );
        }
        let Some(error_type) =
            self.filesystem_support_type("IOError", self.known_std_types.filesystem_io_error)
        else {
            return ControlSignal::missing_checked_fact(
                "filesystem operation requires checked std.fs.IOError",
                boundary.span,
            );
        };
        let value = InterpValue::Nominal {
            ty: error_type,
            value: InterpValue::String(error.to_string().into()).into(),
        };
        self.raise_file_error(error_type, value, boundary.continuation, boundary.span)
    }

    pub(super) fn raise_file_error(
        &mut self,
        error_type: etas_types::TypeId,
        value: InterpValue,
        continuation: Continuation,
        span: Span,
    ) -> ControlSignal {
        self.propagate_perform_signal(PendingPerform {
            residual: None,
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: etas_hir::ResolvedActionRef {
                effect: etas_hir::HirEffectRef {
                    path: etas_hir::unresolved_path_from_segments(&["Error"], span),
                    args: vec![],
                    span,
                },
                action: "raise".into(),
                action_symbol: etas_hir::ResolveResult::Unresolved,
                span,
            },
            error_type: Some(error_type),
            args: vec![value],
            span,
            continuation,
        })
    }
}
