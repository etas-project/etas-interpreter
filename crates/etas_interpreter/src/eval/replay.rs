use std::sync::Arc;

use etas_core::{AnalysisDiagnosticCode, Span};
use etas_host::{
    HostTracePayload, HostTraceRequest,
    replay::{ReplayFingerprint, ReplayKey, ReplayKeyId, ReplayKeyProvider},
};

use crate::{api::RunOptions, control::ExecutionFault};

#[derive(Clone, Debug)]
pub(crate) struct RunReplayKeys {
    provider: Arc<dyn ReplayKeyProvider>,
    key: Arc<ReplayKey>,
}

impl RunReplayKeys {
    pub(crate) fn start(options: &RunOptions) -> Result<Self, String> {
        let provider = options.replay_key_provider.clone();
        provider
            .validate_authority(&options.host_context.authority)
            .map_err(|error| error.to_string())?;
        let key = provider.create().map_err(|error| error.to_string())?;
        Ok(Self { provider, key })
    }

    pub(crate) fn restore(options: &RunOptions, id: ReplayKeyId) -> Result<Self, String> {
        let provider = options.replay_key_provider.clone();
        provider
            .validate_authority(&options.host_context.authority)
            .map_err(|error| error.to_string())?;
        let key = provider.resolve(id).map_err(|error| error.to_string())?;
        if key.id() != id {
            return Err("checkpoint replay-key provider returned a different key identity".into());
        }
        Ok(Self { provider, key })
    }

    pub(crate) fn id(&self) -> ReplayKeyId {
        self.key.id()
    }

    pub(crate) fn retain(&self) -> Result<(), String> {
        self.provider
            .retain(&self.key)
            .map_err(|error| error.to_string())
    }
}

impl super::EvalContext<'_> {
    pub(crate) fn replay_fingerprint(
        &self,
        request: &impl HostTraceRequest,
        id: etas_host::HostRequestId,
        span: Span,
    ) -> Result<ReplayFingerprint, ExecutionFault> {
        self.replay_payload_fingerprint(
            &request.trace_payload().with_field(
                "replay_occurrence",
                etas_host::HostValue::UInt(id.0 as u128),
                etas_host::HostTraceFieldSensitivity::Public,
            ),
            span,
        )
    }

    pub(crate) fn replay_payload_fingerprint(
        &self,
        payload: &HostTracePayload,
        span: Span,
    ) -> Result<ReplayFingerprint, ExecutionFault> {
        self.replay_keys.key.fingerprint(payload).map_err(|error| {
            ExecutionFault::new(
                AnalysisDiagnosticCode::MissingCheckedFact,
                span,
                error.to_string(),
            )
        })
    }
}
