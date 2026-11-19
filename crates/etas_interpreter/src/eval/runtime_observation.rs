//! Execution-owned observations. Etas owns policy, formatting and algorithms.
use super::*;
use etas_std::StdExecutionRole;

impl EvalContext<'_> {
    pub(super) fn execute_runtime_observation(
        &self,
        item: HirItemId,
        role: StdExecutionRole,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        if !args.is_empty() {
            return ControlSignal::missing_checked_fact(
                "runtime observation has a checked zero-argument ABI",
                span,
            );
        }
        if let Some(signal) = self.cancellation_signal(span) {
            return signal;
        }
        if let Err(error) = self.host_context.budget.check_time() {
            return ControlSignal::runtime_fault(error.message, span);
        }
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!()
        };
        if role == StdExecutionRole::CurrentSession {
            let Some(session) = self.current_session.clone() else {
                return ControlSignal::runtime_fault(
                    "current_session requires an active runtime session",
                    span,
                );
            };
            return match super::host_value::decode_checked_host_value(
                HostValue::String(session),
                signature.output,
                self.checked,
            ) {
                Ok(value) => ControlSignal::Value(value),
                Err(reason) => ControlSignal::missing_checked_fact(reason, span),
            };
        }
        let value = if role == StdExecutionRole::StorageNonce {
            match etas_host::execution::fresh_nonce() {
                Ok(nonce) => HostValue::Variant {
                    name: "Ok".into(),
                    fields: vec![HostValue::UInt(nonce)],
                },
                Err(error) => HostValue::Variant {
                    name: "Err".into(),
                    fields: vec![HostValue::Record(vec![
                        (
                            "code".into(),
                            HostValue::String("provider_unavailable".into()),
                        ),
                        ("message".into(), HostValue::String(error.message)),
                    ])],
                },
            }
        } else {
            let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(now) => now,
                Err(error) => {
                    return ControlSignal::runtime_fault(
                        format!("execution clock is before Unix epoch: {error}"),
                        span,
                    );
                }
            };
            match role {
                StdExecutionRole::StreamClock => HostValue::UInt(now.as_millis()),
                StdExecutionRole::StorageContext => {
                    let limits = &self.storage_limits;
                    HostValue::Record(vec![
                        (
                            "max_value_bytes".into(),
                            HostValue::UInt(limits.max_value_bytes as u128),
                        ),
                        (
                            "max_result_bytes".into(),
                            HostValue::UInt(limits.max_result_bytes as u128),
                        ),
                        (
                            "max_nodes".into(),
                            HostValue::UInt(limits.max_nodes as u128),
                        ),
                        (
                            "max_depth".into(),
                            HostValue::UInt(limits.max_depth as u128),
                        ),
                        (
                            "max_scan_rows".into(),
                            HostValue::UInt(limits.max_scan_rows as u128),
                        ),
                        (
                            "max_page_entries".into(),
                            HostValue::UInt(limits.max_page_entries as u128),
                        ),
                        (
                            "receipt_retention_seconds".into(),
                            HostValue::UInt(limits.max_receipt_retention_seconds as u128),
                        ),
                        ("now_seconds".into(), HostValue::UInt(now.as_secs() as u128)),
                    ])
                }
                _ => unreachable!("checked execution observation role"),
            }
        };
        match super::host_value::decode_checked_host_value(value, signature.output, self.checked) {
            Ok(value) => ControlSignal::Value(value),
            Err(error) => ControlSignal::missing_checked_fact(error, span),
        }
    }
}
