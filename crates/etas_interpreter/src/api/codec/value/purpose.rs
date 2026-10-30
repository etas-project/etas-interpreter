use crate::value::InterpValue;

#[derive(Clone, Copy)]
pub(in crate::api::codec) enum ValuePurpose {
    Lossless,
    Report,
}

impl ValuePurpose {
    pub(in crate::api::codec) fn redacts(self, value: &InterpValue) -> bool {
        matches!(self, Self::Report)
            && matches!(
                value,
                InterpValue::Trust {
                    wrapper: etas_types::TrustWrapper::Secret,
                    ..
                }
            )
    }
}
