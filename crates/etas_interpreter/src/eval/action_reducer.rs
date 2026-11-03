use etas_core::Span;
use etas_std::StdActionReducer;

use super::EvalContext;
use crate::{
    control::ControlSignal, intrinsic::dispatch::CheckedStdIntrinsicCall, value::InterpValue,
};

impl EvalContext<'_> {
    pub(super) fn start_action_reducer(
        &mut self,
        reducer: StdActionReducer,
        call: CheckedStdIntrinsicCall,
        args: Vec<InterpValue>,
        span: Span,
    ) -> ControlSignal {
        match reducer {
            StdActionReducer::BoundedDirectoryList {
                entry_limit_parameter,
                byte_limit_parameter,
            } => self.start_directory_list_reducer(
                call,
                args,
                entry_limit_parameter,
                byte_limit_parameter,
                span,
            ),
            StdActionReducer::ReadUntilLimit {
                limit_parameter,
                timeout_parameter,
            } => {
                self.start_stream_read_reducer(call, args, limit_parameter, timeout_parameter, span)
            }
            StdActionReducer::BoundedFileRead { limit_parameter } => {
                self.start_file_read_reducer(call, args, limit_parameter, span)
            }
        }
    }
}
