use super::{CallTargetSnapshot, ContinuationSnapshot, SnapshotValidator, ValueSnapshot};
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, StdIntrinsicIdentity};

impl SnapshotValidator<'_> {
    pub(super) fn file_read_reduction(
        &self,
        state: &ContinuationSnapshot,
        context: &str,
    ) -> Result<(), String> {
        let ContinuationSnapshot::FileReadReduction {
            call, args, limit, ..
        } = state
        else {
            return Err(format!(
                "{context}: expected a bounded file read continuation"
            ));
        };
        self.call_target(call, context)?;
        let CallTargetSnapshot::StdIntrinsic {
            origin,
            substitutions,
            intrinsic,
            dispatch,
            parameter_types,
            result_type,
        } = call
        else {
            return Err(format!(
                "{context}: bounded file read requires a checked intrinsic"
            ));
        };
        let call = CheckedStdIntrinsicCall {
            origin: *origin,
            substitutions: substitutions.clone(),
            identity: StdIntrinsicIdentity {
                intrinsic: *intrinsic,
                dispatch: *dispatch,
            },
            parameter_types: parameter_types.clone(),
            result_type: *result_type,
        };
        let Some(etas_std::StdActionReducer::BoundedFileRead { limit_parameter }) = self
            .dispatch
            .default_action(&call)?
            .and_then(|adapter| adapter.reducer.clone())
        else {
            return Err(format!(
                "{context}: bounded file read reducer is absent from the checked plan"
            ));
        };
        if args.len() != parameter_types.len() {
            return Err(format!(
                "{context}: bounded file read arguments differ from checked ABI"
            ));
        }
        self.snapshot_values(args)?;
        crate::value::contract::validate(
            self,
            args.iter()
                .zip(parameter_types)
                .map(|(value, ty)| crate::value::contract::Seed {
                    value: crate::value::contract::Value::Snapshot(value),
                    ty: *ty,
                }),
            &crate::control::GenericBindings::new(),
        )?;
        let Some(ValueSnapshot::Number(number)) = args.get(limit_parameter) else {
            return Err(format!(
                "{context}: bounded file read byte limit is not a checked usize"
            ));
        };
        if number
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            != Some(*limit)
        {
            return Err(format!(
                "{context}: bounded file read byte limit differs from its checked argument"
            ));
        }
        Ok(())
    }
}
