use std::collections::HashMap;

use etas_builtin::PureIntrinsicRegistry;
use etas_std::{IntrinsicDescriptor, IntrinsicDispatch, LoweringHint, StdIntrinsicId, StdRegistry};

use super::dispatch::{StdCallable, std_callable_for_descriptor};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegisteredStdIntrinsicHandler {
    PureKernel,
    Runtime(StdCallable),
    Host(StdCallable),
    LoweringOnly,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct StdIntrinsicHandlerRegistry {
    handlers: HashMap<StdIntrinsicId, RegisteredStdIntrinsicHandler>,
}

impl StdIntrinsicHandlerRegistry {
    pub(crate) fn build(std_registry: &StdRegistry) -> Self {
        let pure_registry = PureIntrinsicRegistry;
        let mut handlers = HashMap::new();
        for descriptor in std_registry
            .symbols()
            .filter_map(|symbol| symbol.intrinsic.as_ref())
        {
            if handlers.contains_key(&descriptor.id) {
                continue;
            }
            let handler = match descriptor.dispatch {
                IntrinsicDispatch::PureKernel if pure_registry.contains(descriptor.id) => {
                    Some(RegisteredStdIntrinsicHandler::PureKernel)
                }
                IntrinsicDispatch::Runtime => std_callable_for_descriptor(descriptor)
                    .map(RegisteredStdIntrinsicHandler::Runtime),
                IntrinsicDispatch::Host => {
                    std_callable_for_descriptor(descriptor).map(RegisteredStdIntrinsicHandler::Host)
                }
                IntrinsicDispatch::LoweringOnly => {
                    Some(RegisteredStdIntrinsicHandler::LoweringOnly)
                }
                IntrinsicDispatch::PureKernel => None,
            };
            if let Some(handler) = handler {
                handlers.insert(descriptor.id, handler);
            }
        }
        Self { handlers }
    }

    pub(crate) fn validate_descriptor(
        &self,
        descriptor: &IntrinsicDescriptor,
    ) -> Result<(), String> {
        if descriptor.dispatch == IntrinsicDispatch::LoweringOnly
            && descriptor.lowering == LoweringHint::None
        {
            return Err(format!(
                "standard intrinsic `{}` ({}) is lowering-only but has no lowering marker",
                descriptor.qualified_path.join("."),
                descriptor.id.0
            ));
        }
        let Some(handler) = self.handlers.get(&descriptor.id) else {
            return Err(format!(
                "standard intrinsic `{}` ({}) has no registered {:?} handler",
                descriptor.qualified_path.join("."),
                descriptor.id.0,
                descriptor.dispatch
            ));
        };
        if handler.dispatch() != descriptor.dispatch {
            return Err(format!(
                "standard intrinsic `{}` ({}) declares {:?} dispatch but its registered handler is {:?}",
                descriptor.qualified_path.join("."),
                descriptor.id.0,
                descriptor.dispatch,
                handler.dispatch()
            ));
        }
        Ok(())
    }

    pub(crate) fn executable(
        &self,
        intrinsic: StdIntrinsicId,
        dispatch: IntrinsicDispatch,
    ) -> Result<StdCallable, String> {
        let Some(handler) = self.handlers.get(&intrinsic) else {
            return Err(format!(
                "standard intrinsic {} has no registered handler",
                intrinsic.0
            ));
        };
        if handler.dispatch() != dispatch {
            return Err(format!(
                "standard intrinsic {} dispatch mismatch: checkpoint/call target declares {:?}, current handler is {:?}",
                intrinsic.0,
                dispatch,
                handler.dispatch()
            ));
        }
        match handler {
            RegisteredStdIntrinsicHandler::Runtime(callable)
            | RegisteredStdIntrinsicHandler::Host(callable) => Ok(callable.clone()),
            RegisteredStdIntrinsicHandler::PureKernel => Err(format!(
                "standard intrinsic {} is a pure kernel and requires checked ABI facts",
                intrinsic.0
            )),
            RegisteredStdIntrinsicHandler::LoweringOnly => Err(format!(
                "standard intrinsic {} is lowering-only and cannot execute as a runtime call",
                intrinsic.0
            )),
        }
    }

    pub(crate) fn contains(&self, intrinsic: StdIntrinsicId, dispatch: IntrinsicDispatch) -> bool {
        self.handlers
            .get(&intrinsic)
            .is_some_and(|handler| handler.dispatch() == dispatch)
    }
}

impl RegisteredStdIntrinsicHandler {
    fn dispatch(&self) -> IntrinsicDispatch {
        match self {
            Self::PureKernel => IntrinsicDispatch::PureKernel,
            Self::Runtime(_) => IntrinsicDispatch::Runtime,
            Self::Host(_) => IntrinsicDispatch::Host,
            Self::LoweringOnly => IntrinsicDispatch::LoweringOnly,
        }
    }
}
