use crate::value::InterpValue;

#[derive(Clone, Copy)]
pub(in crate::api::codec) enum ValuePurpose {
    Lossless,
    Report,
}

impl ValuePurpose {
    pub(in crate::api::codec) fn redacts_wrapper(self, wrapper: etas_types::TrustWrapper) -> bool {
        matches!(self, Self::Report) && wrapper == etas_types::TrustWrapper::Secret
    }

    pub(in crate::api::codec) fn redacts(self, value: &InterpValue) -> bool {
        matches!(value, InterpValue::Trust { wrapper, .. } if self.redacts_wrapper(*wrapper))
    }
}
