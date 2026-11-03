use super::EvalContext;
use crate::{
    control::{
        Continuation, ControlSignal, DefaultActionBoundary, DirectoryListReduction,
        HostBoundaryDecode, HostBoundaryRequest, PendingHostBoundary,
    },
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    value::InterpValue,
};
use etas_core::Span;
use etas_host::{FilesystemOperation, FilesystemRequest, HostValue, WorkspacePathRef};

pub(crate) fn listing_within_bounds(
    value: &InterpValue,
    parent: &WorkspacePathRef,
    max_entries: usize,
    max_bytes: usize,
) -> Result<bool, String> {
    let InterpValue::List(entries) = value else {
        return Err("bounded directory action returned a non-list value".into());
    };
    if entries.len() > max_entries {
        return Ok(false);
    }
    let expected_parent = if parent.relative.as_os_str() == "." {
        std::path::Path::new("")
    } else {
        &parent.relative
    };
    let mut bytes = 0usize;
    let mut names = std::collections::BTreeSet::new();
    for value in entries.iter() {
        let InterpValue::WorkspacePath(path) = value else {
            return Err("directory entry is not a checked workspace path".into());
        };
        let normalized = WorkspacePathRef::new(path.region.clone(), &path.relative)
            .map_err(|error| error.message)?;
        if normalized != *path
            || path.region != parent.region
            || path.relative.parent() != Some(expected_parent)
            || !names.insert(&path.relative)
        {
            return Err("directory entries have inconsistent region, parent, normalization or duplicate identity".into());
        }
        let name = path.relative_name().map_err(|error| error.message)?;
        let Some(next) = bytes
            .checked_add(name.len())
            .filter(|bytes| *bytes <= max_bytes)
        else {
            return Ok(false);
        };
        bytes = next;
    }
    Ok(true)
}

impl EvalContext<'_> {
    pub(super) fn start_directory_list_reducer(
        &mut self,
        call: CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        entry_parameter: usize,
        byte_parameter: usize,
        span: Span,
    ) -> ControlSignal {
        let limits = (|| {
            let entries = args
                .get(entry_parameter)
                .ok_or("directory listing lacks its entry limit")?;
            let bytes = args
                .get(byte_parameter)
                .ok_or("directory listing lacks its byte limit")?;
            let entries = self
                .usize_argument(entries, "directory entry limit", span)
                .map_err(|fault| fault.message)?;
            let bytes = self
                .usize_argument(bytes, "directory byte limit", span)
                .map_err(|fault| fault.message)?;
            Ok::<_, String>((entries, bytes))
        })();
        let (max_entries, max_bytes) = match limits {
            Ok(limits) => limits,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        let (proof, payload) = match self.prepare_native_action(&call, &args, span) {
            Ok(Some(invocation)) => invocation,
            Ok(None) => {
                return ControlSignal::missing_checked_fact(
                    "bounded listing lacks its checked action invocation",
                    span,
                );
            }
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let [path] = payload.as_slice() else {
            return ControlSignal::missing_checked_fact(
                "bounded listing requires one path payload",
                span,
            );
        };
        let boundary = DefaultActionBoundary::DirectoryListLimited {
            path: path.clone(),
            max_entries,
            max_bytes,
            span,
        };
        let signal = self.mediate_native_action(proof, payload, boundary, span);
        self.continue_chain_signal(
            signal,
            Continuation::DirectoryListReduction(Box::new(DirectoryListReduction {
                call,
                args,
                max_entries,
                max_bytes,
                span,
            })),
        )
    }

    pub(super) fn continue_directory_list(
        &mut self,
        state: DirectoryListReduction,
        value: InterpValue,
    ) -> ControlSignal {
        let Some(InterpValue::WorkspacePath(parent)) = state.args.first() else {
            return ControlSignal::missing_checked_fact(
                "bounded listing lost its parent path",
                state.span,
            );
        };
        match listing_within_bounds(&value, parent, state.max_entries, state.max_bytes) {
            Ok(true) => {
                let InterpValue::List(entries) = value else {
                    return ControlSignal::missing_checked_fact(
                        "directory reducer lost its validated list",
                        state.span,
                    );
                };
                let mut ordered = Vec::new();
                for entry in entries.iter() {
                    let InterpValue::WorkspacePath(path) = entry else {
                        return ControlSignal::missing_checked_fact(
                            "directory reducer lost a validated path",
                            state.span,
                        );
                    };
                    let name = match path.relative_name() {
                        Ok(name) => name,
                        Err(error) => {
                            return ControlSignal::missing_checked_fact(error.message, state.span);
                        }
                    };
                    ordered.push((name.to_owned(), entry.clone()));
                }
                ordered.sort_by(|left, right| left.0.cmp(&right.0));
                ControlSignal::Value(InterpValue::List(crate::value::ListValue::new(
                    ordered.into_iter().map(|(_, value)| value).collect(),
                )))
            }
            Ok(false) => self.raise_directory_list_limit(
                state.max_entries,
                state.max_bytes,
                Continuation::BlockValue,
                state.span,
            ),
            Err(reason) => ControlSignal::missing_checked_fact(reason, state.span),
        }
    }

    pub(super) fn execute_directory_list_default(
        &mut self,
        path: InterpValue,
        max_entries: usize,
        max_bytes: usize,
        span: Span,
    ) -> ControlSignal {
        let path = match self.workspace_path(&path, span) {
            Ok(path) => path,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Filesystem(FilesystemRequest {
                id: self.next_host_request_id(),
                operation: FilesystemOperation::ReadDirLimited {
                    path,
                    max_entries,
                    max_bytes,
                },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode: HostBoundaryDecode::DirectoryListLimited {
                max_entries,
                max_bytes,
            },
            span,
            continuation: Continuation::BlockValue,
        })
    }

    pub(crate) fn raise_directory_list_limit(
        &mut self,
        max_entries: usize,
        max_bytes: usize,
        continuation: Continuation,
        span: Span,
    ) -> ControlSignal {
        let Some(ty) = self
            .checked
            .types
            .known_std_types
            .directory_list_limit_error
        else {
            return ControlSignal::missing_checked_fact(
                "bounded listing requires checked std.fs.ListLimitError",
                span,
            );
        };
        let value = match super::host_to_checked_interp_value(
            HostValue::Variant {
                name: "LimitExceeded".into(),
                fields: vec![
                    HostValue::UInt(max_entries as u128),
                    HostValue::UInt(max_bytes as u128),
                ],
            },
            ty,
            self.checked,
            &self.storage_limits,
        ) {
            Ok(value) => value,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        self.raise_file_error(ty, value, continuation, span)
    }
}
