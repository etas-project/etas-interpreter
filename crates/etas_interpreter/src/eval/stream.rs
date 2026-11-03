use std::time::{SystemTime, UNIX_EPOCH};

use etas_core::{AnalysisDiagnosticCode, Span};
use etas_host::{StreamFailure, StreamOperation, StreamRequest};

use super::EvalContext;
use crate::{
    control::{
        Continuation, ControlSignal, DefaultActionBoundary, ExecutionFault, HostBoundaryDecode,
        HostBoundaryRequest, PendingHostBoundary, StreamReadReduction,
    },
    intrinsic::dispatch::CheckedStdIntrinsicCall,
    value::InterpValue,
};

impl EvalContext<'_> {
    pub(super) fn start_stream_read_reducer(
        &mut self,
        call: CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        limit_parameter: usize,
        timeout_parameter: usize,
        span: Span,
    ) -> ControlSignal {
        let prepare = || {
            let limit = args
                .get(limit_parameter)
                .ok_or_else(|| missing(span, "stream reducer has no checked byte limit"))?;
            let limit = self.usize_argument(limit, "stream byte limit", span)?;
            let timeout = args
                .get(timeout_parameter)
                .ok_or_else(|| missing(span, "stream reducer has no checked timeout"))?;
            let deadline_unix_ms = self
                .optional_timeout_ms(Some(timeout), span)?
                .map(|timeout| {
                    unix_ms(span)?
                        .checked_add(timeout)
                        .ok_or_else(|| missing(span, "stream deadline overflows Unix milliseconds"))
                })
                .transpose()?;
            Ok::<_, ExecutionFault>(StreamReadReduction {
                call,
                args,
                limit,
                deadline_unix_ms,
                body: Default::default(),
                span,
            })
        };
        match prepare() {
            Ok(state) => self.next_stream_read(state),
            Err(fault) => ControlSignal::Fault(Box::new(fault)),
        }
    }

    pub(super) fn continue_stream_read(
        &mut self,
        mut state: StreamReadReduction,
        value: InterpValue,
    ) -> ControlSignal {
        match remaining_timeout(&state) {
            Ok(Some(0)) => {
                return self.raise_stream_failure(
                    StreamFailure::TimedOut,
                    Continuation::BlockValue,
                    state.span,
                );
            }
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
            _ => {}
        }
        let proof = match self.prepare_native_action(&state.call, &state.args, state.span) {
            Ok(Some((proof, _))) => proof,
            Ok(None) => {
                return ControlSignal::missing_checked_fact(
                    "stream reducer has no checked action output",
                    state.span,
                );
            }
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let (name, fields) =
            match crate::value::enum_value::payload(&value, proof.output, self.checked) {
                Ok(payload) => payload,
                Err(reason) => return ControlSignal::missing_checked_fact(reason, state.span),
            };
        match (name, fields) {
            ("Eof", []) => ControlSignal::Value(InterpValue::Bytes(state.body)),
            ("Data", [InterpValue::Bytes(chunk)]) => {
                let remaining = match remaining_capacity(&state) {
                    Ok(remaining) => remaining,
                    Err(fault) => return ControlSignal::Fault(Box::new(fault)),
                };
                if chunk.len() > remaining {
                    return self.raise_stream_failure(
                        StreamFailure::LimitExceeded {
                            limit_bytes: state.limit,
                        },
                        Continuation::BlockValue,
                        state.span,
                    );
                }
                let mut body = state.body.into_vec();
                if body.try_reserve(chunk.len()).is_err() {
                    return self.raise_stream_failure(
                        StreamFailure::Host(etas_host::HostError::new(
                            etas_host::HostErrorCode::ProviderUnavailable,
                            "stream result allocation exceeds available memory",
                        )),
                        Continuation::BlockValue,
                        state.span,
                    );
                }
                body.extend_from_slice(chunk);
                state.body = body.into();
                self.next_stream_read(state)
            }
            _ => ControlSignal::missing_checked_fact(
                "stream action returned an invalid StreamRead payload",
                state.span,
            ),
        }
    }

    fn next_stream_read(&mut self, state: StreamReadReduction) -> ControlSignal {
        let timeout_ms = match remaining_timeout(&state) {
            Ok(Some(0)) => {
                return self.raise_stream_failure(
                    StreamFailure::TimedOut,
                    Continuation::BlockValue,
                    state.span,
                );
            }
            Ok(timeout) => timeout,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let (proof, args) = match self.prepare_native_action(&state.call, &state.args, state.span) {
            Ok(Some(invocation)) => invocation,
            Ok(None) => {
                return ControlSignal::missing_checked_fact(
                    "stream reducer has no checked action invocation",
                    state.span,
                );
            }
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let [stream] = args.as_slice() else {
            return ControlSignal::missing_checked_fact(
                "stream reducer requires one checked stream payload",
                state.span,
            );
        };
        // Probe beyond an exactly full buffer so reaching the limit cannot
        // silently truncate a stream that has not reached EOF.
        let max_bytes = match remaining_capacity(&state).and_then(|remaining| {
            remaining
                .checked_add(1)
                .map(|count| count.min(8192))
                .ok_or_else(|| {
                    missing(
                        state.span,
                        "stream read probe exceeds addressable byte count",
                    )
                })
        }) {
            Ok(max_bytes) => max_bytes,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let boundary = DefaultActionBoundary::StreamRead {
            stream: stream.clone(),
            max_bytes,
            timeout_ms,
            span: state.span,
        };
        let signal = self.mediate_native_action(proof, args, boundary, state.span);
        self.continue_chain_signal(signal, Continuation::StreamReadReduction(Box::new(state)))
    }

    pub(super) fn execute_stream_read_default(
        &mut self,
        stream: InterpValue,
        max_bytes: usize,
        timeout_ms: Option<u64>,
        result_type: etas_types::TypeId,
        span: Span,
    ) -> ControlSignal {
        let stream = match self.stream_ref_argument(&stream, span) {
            Ok(stream) => stream,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        ControlSignal::pending_host(PendingHostBoundary {
            request: HostBoundaryRequest::Stream(StreamRequest {
                id: self.next_host_request_id(),
                operation: StreamOperation::Read {
                    stream,
                    max_bytes,
                    timeout_ms,
                },
                authority: self.host_authority(),
                trace: self.host_trace(),
                budget: self.host_budget(),
            }),
            decode: HostBoundaryDecode::StreamRead { result_type },
            span,
            continuation: Continuation::BlockValue,
        })
    }
}

fn missing(span: Span, message: &str) -> ExecutionFault {
    ExecutionFault::new(AnalysisDiagnosticCode::MissingCheckedFact, span, message)
}

fn unix_ms(span: Span) -> Result<u64, ExecutionFault> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .ok_or_else(|| missing(span, "stream deadline requires a representable Unix clock"))
}

fn remaining_timeout(state: &StreamReadReduction) -> Result<Option<u64>, ExecutionFault> {
    state
        .deadline_unix_ms
        .map(|deadline| unix_ms(state.span).map(|now| deadline.saturating_sub(now)))
        .transpose()
}

fn remaining_capacity(state: &StreamReadReduction) -> Result<usize, ExecutionFault> {
    state.limit.checked_sub(state.body.len()).ok_or_else(|| {
        missing(
            state.span,
            "stream reducer buffer exceeds its checked byte limit",
        )
    })
}
