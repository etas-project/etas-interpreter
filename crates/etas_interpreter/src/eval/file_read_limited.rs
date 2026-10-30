use etas_core::Span;
use etas_host::{FilesystemOperation, FilesystemRequest, HostValue};

use super::EvalContext;
use crate::{
    control::{
        Continuation, ControlSignal, DefaultActionBoundary, FileReadReduction, HostBoundaryDecode,
        HostBoundaryRequest, PendingHostBoundary,
    },
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    value::InterpValue,
};

impl EvalContext<'_> {
    pub(super) fn start_file_read_reducer(
        &mut self,
        call: CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        limit_parameter: usize,
        span: Span,
    ) -> ControlSignal {
        let Some(limit) = args.get(limit_parameter) else {
            return ControlSignal::missing_checked_fact(
                "bounded file read has no checked byte limit",
                span,
            );
        };
        let limit = match self.usize_argument(limit, "bounded file read maximum", span) {
            Ok(limit) => limit,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let (proof, payload) = match self.prepare_native_action(&call, &args, span) {
            Ok(Some(invocation)) => invocation,
            Ok(None) => {
                return ControlSignal::missing_checked_fact(
                    "bounded file read has no checked action invocation",
                    span,
                );
            }
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let [path] = payload.as_slice() else {
            return ControlSignal::missing_checked_fact(
                "bounded file read requires one checked path payload",
                span,
            );
        };
        let boundary = DefaultActionBoundary::FileReadLimited {
            path: path.clone(),
            max_bytes: limit,
            span,
        };
        let signal = self.mediate_native_action(proof, payload, boundary, span);
        self.continue_chain_signal(
            signal,
            Continuation::FileReadReduction(Box::new(FileReadReduction {
                call,
                args,
                limit,
                span,
            })),
        )
    }

    pub(super) fn continue_file_read(
        &mut self,
        state: FileReadReduction,
        value: InterpValue,
    ) -> ControlSignal {
        let InterpValue::Bytes(bytes) = value else {
            return ControlSignal::missing_checked_fact(
                "bounded file action returned a non-bytes payload",
                state.span,
            );
        };
        if bytes.len() > state.limit {
            self.raise_file_read_limit(state.limit, Continuation::BlockValue, state.span)
        } else {
            ControlSignal::Value(InterpValue::Bytes(bytes))
        }
    }

    pub(super) fn execute_file_read_default(
        &mut self,
        path: InterpValue,
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
                operation: FilesystemOperation::ReadLimited { path, max_bytes },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode: HostBoundaryDecode::FileReadLimited { max_bytes },
            span,
            continuation: Continuation::BlockValue,
        })
    }

    pub(crate) fn raise_file_read_limit(
        &mut self,
        max_bytes: usize,
        continuation: Continuation,
        span: Span,
    ) -> ControlSignal {
        let Some(error_type) = self.known_std_types.file_read_limit_error else {
            return ControlSignal::missing_checked_fact(
                "bounded read requires checked std.fs.ReadLimitError",
                span,
            );
        };
        let value = match super::host_to_checked_interp_value(
            HostValue::Variant {
                name: "LimitExceeded".into(),
                fields: vec![HostValue::UInt(max_bytes as u128)],
            },
            error_type,
            self.checked,
            &self.storage_limits,
        ) {
            Ok(value) => value,
            Err(reason) => return ControlSignal::missing_checked_fact(reason, span),
        };
        self.raise_file_error(error_type, value, continuation, span)
    }
}
