use super::*;
use etas_host::StreamFailure;

impl<'a> EvalContext<'a> {
    pub(super) fn enter_stream_read_scope(
        &self,
        item: HirItemId,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        let fail = |reason: &str| ControlSignal::missing_checked_fact(reason, span);
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            return fail("read scope requires its checked source signature");
        };
        let (Some(etas_types::Type::Tuple(fields)), [stream]) =
            (self.checked.type_store.get(signature.output), args)
        else {
            return fail("read scope requires a stream and its checked (stream, owner) result");
        };
        let [_, owner_option] = fields.as_slice() else {
            return fail("read scope result has no owner slot");
        };
        let Some(owner_type) = etas_types::value_representation::option_parameter(
            &self.checked.type_store,
            &self.checked.types.known_std_types,
            *owner_option,
        ) else {
            return fail("read scope requires its checked optional owner type");
        };
        let mut scoped = stream.clone();
        let mut owner = None;
        if let InterpValue::Nominal { ty, value } = stream
            && let InterpValue::Record(record) = value.as_ref()
            && let Some(InterpValue::HostHandle(handle)) = record.get_ref("connection")
            && let Some(native) = handle.byte_stream_ref()
        {
            let scope = etas_host::StreamReadScope::new(
                &native,
                format!("{:?}", self.task.logical_identity),
            );
            let scoped_handle = handle
                .with_read_scope(&scope)
                .expect("checked native byte stream");
            let mut fields = record.snapshot();
            fields
                .iter_mut()
                .find(|(name, _)| name == "connection")
                .unwrap()
                .1 = InterpValue::HostHandle(scoped_handle);
            scoped = InterpValue::Nominal {
                ty: *ty,
                value: InterpValue::Record(fields.into()).into(),
            };
            owner = Some(InterpValue::HostHandle(
                crate::value::HostHandleValue::read_owner(owner_type, scope),
            ));
        }
        let (variant, fields) = match owner {
            Some(owner) => ("Some", vec![owner]),
            None => ("None", vec![]),
        };
        match self
            .plan
            .prepared_program()
            .construct_enum(*owner_option, variant, fields)
        {
            Ok(owner) => ControlSignal::Value(InterpValue::Tuple(vec![scoped, owner].into())),
            Err(reason) => fail(&reason),
        }
    }

    pub(crate) fn stream_failure_signal(
        &mut self,
        host: PendingHostBoundary,
        failure: StreamFailure,
    ) -> ControlSignal {
        self.raise_stream_failure(failure, host.continuation, host.span)
    }

    pub(super) fn raise_stream_failure(
        &mut self,
        failure: StreamFailure,
        continuation: Continuation,
        span: Span,
    ) -> ControlSignal {
        let Some(error_type) = self.known_std_types.stream_error else {
            return ControlSignal::missing_checked_fact(
                "stream host failure requires checked std.stream StreamError type",
                span,
            );
        };
        let raw = stream_error_value(failure);
        let value = if self.checked.hir.source_standard_types {
            InterpValue::Nominal {
                ty: error_type,
                value: raw.into(),
            }
        } else {
            raw
        };
        let perform = PendingPerform {
            checked_action: None,
            default_boundary: None,
            forwarded_call: None,
            inference: None,
            expr: None,
            action: stream_error_raise_action(span),
            error_type: Some(error_type),
            args: vec![value],
            span,
            continuation,
            residual: None,
        };
        self.propagate_perform_signal(perform)
    }
}

fn stream_error_raise_action(span: Span) -> ResolvedActionRef {
    ResolvedActionRef {
        effect: etas_hir::HirEffectRef {
            path: etas_hir::unresolved_path_from_segments(&["Error"], span),
            args: Vec::new(),
            span,
        },
        action: "raise".to_owned(),
        action_symbol: ResolveResult::Unresolved,
        span,
    }
}

fn stream_error_value(failure: StreamFailure) -> InterpValue {
    match failure {
        StreamFailure::Busy => InterpValue::Variant {
            name: "Busy".into(),
            fields: Vec::new().into(),
        },
        StreamFailure::LimitExceeded { .. } => InterpValue::Variant {
            name: "LimitExceeded".to_owned().into(),
            fields: Vec::new().into(),
        },
        StreamFailure::TimedOut => InterpValue::Variant {
            name: "TimedOut".to_owned().into(),
            fields: Vec::new().into(),
        },
        StreamFailure::Cancelled => InterpValue::Variant {
            name: "Cancelled".to_owned().into(),
            fields: Vec::new().into(),
        },
        StreamFailure::Closed => InterpValue::Variant {
            name: "Closed".to_owned().into(),
            fields: Vec::new().into(),
        },
        StreamFailure::Interrupted => InterpValue::Variant {
            name: "Interrupted".to_owned().into(),
            fields: Vec::new().into(),
        },
        StreamFailure::Host(error) => InterpValue::Variant {
            name: "Host".to_owned().into(),
            fields: vec![InterpValue::String(
                format!(
                    "{}: {}; details={:?}",
                    error.code.as_str(),
                    error.message,
                    error.details
                )
                .into(),
            )]
            .into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use etas_host::{HostError, HostErrorCode, StreamFailure};

    use super::stream_error_value;
    use crate::value::InterpValue;

    #[test]
    fn stream_error_mapping_uses_typed_code_not_message_text() {
        assert_eq!(
            stream_error_value(StreamFailure::Host(HostError::new(
                HostErrorCode::ProviderUnavailable,
                "closed, cancelled, interrupted, and timed out",
            ))),
            InterpValue::Variant {
                name: "Host".to_owned().into(),
                fields: vec![InterpValue::String(
                    "ProviderUnavailable: closed, cancelled, interrupted, and timed out; details=[]"
                        .to_owned().into(),
                )]
                .into(),
            }
        );
        assert_eq!(
            stream_error_value(StreamFailure::TimedOut),
            InterpValue::Variant {
                name: "TimedOut".to_owned().into(),
                fields: Vec::new().into(),
            }
        );
    }

    #[test]
    fn stream_error_mapping_preserves_all_typed_variants() {
        let cases = [
            (StreamFailure::Busy, "Busy"),
            (
                StreamFailure::LimitExceeded { limit_bytes: 1 },
                "LimitExceeded",
            ),
            (StreamFailure::TimedOut, "TimedOut"),
            (StreamFailure::Cancelled, "Cancelled"),
            (StreamFailure::Closed, "Closed"),
            (StreamFailure::Interrupted, "Interrupted"),
        ];
        for (failure, expected) in cases {
            assert_eq!(
                stream_error_value(failure),
                InterpValue::Variant {
                    name: expected.to_owned().into(),
                    fields: Vec::new().into(),
                }
            );
        }
    }
}
