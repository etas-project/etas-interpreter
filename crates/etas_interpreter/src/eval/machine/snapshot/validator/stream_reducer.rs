use super::{CallTargetSnapshot, ContinuationSnapshot, SnapshotValidator, TypeId, ValueSnapshot};
use crate::intrinsic::dispatch::{CheckedStdIntrinsicCall, StdIntrinsicIdentity};

impl SnapshotValidator<'_> {
    pub(super) fn stream_read_reduction(
        &self,
        state: &ContinuationSnapshot,
        context: &str,
    ) -> Result<(), String> {
        let ContinuationSnapshot::StreamReadReduction {
            call,
            args,
            limit,
            deadline_unix_ms,
            body,
            ..
        } = state
        else {
            return Err(format!("{context}: expected a stream reducer continuation"));
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
                "{context}: stream reducer requires a checked standard intrinsic"
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
        let Some(etas_std::StdActionReducer::ReadUntilLimit {
            limit_parameter,
            timeout_parameter,
        }) = self
            .dispatch
            .default_action(&call)?
            .and_then(|adapter| adapter.reducer.clone())
        else {
            return Err(format!(
                "{context}: stream reducer is absent from the current plan"
            ));
        };
        if args.len() != parameter_types.len() {
            return Err(format!(
                "{context}: stream reducer argument count differs from its checked ABI"
            ));
        }
        self.snapshot_values(args)?;
        self.snapshot_value(body)?;
        let limit_value = args
            .get(limit_parameter)
            .ok_or("stream reducer byte limit is missing")?;
        if config_integer(limit_value, "bytes", parameter_types[limit_parameter])
            != u64::try_from(*limit).ok()
        {
            return Err(format!(
                "{context}: stream reducer byte limit differs from its checked configuration"
            ));
        }
        let ValueSnapshot::Bytes(body) = body else {
            return Err(format!("{context}: stream reducer buffer must be bytes"));
        };
        if body.len() > *limit {
            return Err(format!(
                "{context}: stream reducer buffer exceeds its byte limit"
            ));
        }
        let timeout = args
            .get(timeout_parameter)
            .ok_or("stream reducer timeout is missing")?;
        let timeout_valid = match (timeout, deadline_unix_ms) {
            (ValueSnapshot::OptionNone, None) => true,
            (ValueSnapshot::OptionSome(value), Some(_)) => match self
                .checked
                .type_store
                .get(parameter_types[timeout_parameter])
            {
                Some(etas_types::Type::Option(ty)) => config_integer(value, "ms", *ty).is_some(),
                _ => false,
            },
            _ => false,
        };
        if !timeout_valid {
            return Err(format!(
                "{context}: stream reducer deadline differs from its timeout configuration"
            ));
        }
        Ok(())
    }
}

fn config_integer(value: &ValueSnapshot, field: &str, expected: TypeId) -> Option<u64> {
    let ValueSnapshot::Nominal { ty, value } = value else {
        return None;
    };
    if *ty != expected {
        return None;
    }
    let ValueSnapshot::Record(fields) = &**value else {
        return None;
    };
    let [(name, ValueSnapshot::Number(number))] = fields.as_slice() else {
        return None;
    };
    (name == field).then(|| number.as_u64()).flatten()
}
