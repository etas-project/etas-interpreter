use super::{CallTargetSnapshot, ContinuationSnapshot, SnapshotValidator, ValueSnapshot};
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, StdIntrinsicIdentity};

impl SnapshotValidator<'_> {
    pub(super) fn bounded_filesystem_reduction(
        &self,
        state: &ContinuationSnapshot,
        context: &str,
    ) -> Result<(), String> {
        let (call, args) = match state {
            ContinuationSnapshot::FileReadReduction { call, args, .. }
            | ContinuationSnapshot::DirectoryListReduction { call, args, .. } => (call, args),
            _ => {
                return Err(format!(
                    "{context}: expected a bounded filesystem continuation"
                ));
            }
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
                "{context}: bounded filesystem operation requires a checked intrinsic"
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
        let reducer = self
            .dispatch
            .default_action(&call)?
            .and_then(|adapter| adapter.reducer.clone());
        let limits = match (state, reducer) {
            (
                ContinuationSnapshot::FileReadReduction { limit, .. },
                Some(etas_std::StdActionReducer::BoundedFileRead { limit_parameter }),
            ) => vec![(limit_parameter, *limit)],
            (
                ContinuationSnapshot::DirectoryListReduction {
                    max_entries,
                    max_bytes,
                    ..
                },
                Some(etas_std::StdActionReducer::BoundedDirectoryList {
                    entry_limit_parameter,
                    byte_limit_parameter,
                }),
            ) => vec![
                (entry_limit_parameter, *max_entries),
                (byte_limit_parameter, *max_bytes),
            ],
            _ => {
                return Err(format!(
                    "{context}: bounded filesystem reducer is absent from the checked plan"
                ));
            }
        };
        if args.len() != parameter_types.len() {
            return Err(format!(
                "{context}: bounded filesystem arguments differ from checked ABI"
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
        for (parameter, limit) in limits {
            let Some(ValueSnapshot::Number(number)) = args.get(parameter) else {
                return Err(format!(
                    "{context}: bounded filesystem limit is not a checked usize"
                ));
            };
            if number
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                != Some(limit)
            {
                return Err(format!(
                    "{context}: bounded filesystem limit differs from its checked argument"
                ));
            }
        }
        Ok(())
    }
}
