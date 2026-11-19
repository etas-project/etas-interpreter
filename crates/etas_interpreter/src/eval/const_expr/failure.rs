use crate::control::{ControlSignal, ExecutionFault};

/// Source literal constructors execute on the VM and can observe cancellation.
/// Preserve that outcome through recursive constant evaluation.
pub(crate) enum ConstantFailure {
    Fault(ExecutionFault),
    Cancelled(etas_host::execution::CancellationCause),
}

impl From<ExecutionFault> for ConstantFailure {
    fn from(fault: ExecutionFault) -> Self {
        Self::Fault(fault)
    }
}

impl ConstantFailure {
    pub(crate) fn into_signal(self) -> ControlSignal {
        match self {
            Self::Fault(fault) => ControlSignal::Fault(Box::new(fault)),
            Self::Cancelled(cause) => ControlSignal::Cancelled(cause),
        }
    }
}
