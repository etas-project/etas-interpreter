use etas_host::{HostError, HostErrorCode, HostRequestKind, HostValue, PolicySubject};

use crate::{
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
};

use super::{
    host_dispatch::HostDispatch,
    policy::{boundary_policy_ref_for, evaluate_before_boundary},
};

pub(in crate::driver) async fn dispatch(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    pending: &crate::control::PendingModel,
    machine: &mut EvalMachine,
) -> bool {
    let policy_ref = boundary_policy_ref_for(eval, pending.request.policy_ref.clone());
    if !evaluate_before_boundary(
        eval,
        host,
        policy_ref,
        policy_subject(&pending.request),
        pending.span,
        "model",
    )
    .await
    {
        return false;
    }

    if let Err(error) = pending.request.budget.check_time() {
        machine.resume_model_result(Err(error));
        return true;
    }
    let remaining_tokens = match pending.request.budget.remaining_tokens() {
        Ok(remaining) => remaining,
        Err(error) => {
            machine.resume_model_result(Err(error));
            return true;
        }
    };
    let reservation_amount = remaining_tokens
        .or(pending.request.options.max_output_tokens)
        .unwrap_or(0);
    let reservation = match pending.request.budget.reserve_tokens(reservation_amount) {
        Ok(reservation) => reservation,
        Err(error) => {
            machine.resume_model_result(Err(error));
            return true;
        }
    };
    let remaining_cost = match pending.request.budget.remaining_cost() {
        Ok(remaining) => remaining,
        Err(error) => {
            let error = pending
                .request
                .budget
                .release_tokens(reservation)
                .err()
                .unwrap_or(error);
            machine.resume_model_result(Err(error));
            return true;
        }
    };
    let cost_reservation = match remaining_cost.as_ref() {
        Some(cost) => match pending
            .request
            .budget
            .reserve_cost(cost.max_micros, &cost.currency)
        {
            Ok(reservation) => Some(reservation),
            Err(error) => {
                let error = pending
                    .request
                    .budget
                    .release_tokens(reservation)
                    .err()
                    .unwrap_or(error);
                machine.resume_model_result(Err(error));
                return true;
            }
        },
        None => None,
    };

    let request_id = pending.request.id;
    let result = match HostDispatch::execute(
        eval,
        request_id,
        HostRequestKind::Model,
        pending.request.authority.clone(),
        pending.request.trace.clone(),
        host.model(pending.request.clone()),
    )
    .await
    {
        Ok(response) => {
            let consumed = match response.usage.as_ref() {
                Some(usage) => usage
                    .input_tokens
                    .checked_add(usage.output_tokens)
                    .ok_or_else(|| {
                        HostError::new(
                            HostErrorCode::InvalidResponse,
                            "model usage token count overflowed",
                        )
                    }),
                None if remaining_tokens.is_some() || remaining_cost.is_some() => {
                    Err(HostError::new(
                        HostErrorCode::InvalidResponse,
                        "model response omitted usage required by the run-owned execution budget",
                    ))
                }
                None => Ok(0),
            };
            let cost_settlement = match (&remaining_cost, response.usage.as_ref()) {
                (Some(expected), Some(usage)) => match usage.cost.as_ref() {
                    Some(cost) if cost.currency == expected.currency => Ok(cost_reservation
                        .clone()
                        .map(|reservation| (reservation, cost.micros))),
                    Some(cost) => Err(HostError::new(
                        HostErrorCode::InvalidResponse,
                        "model usage cost currency does not match the execution budget",
                    )
                    .with_detail("expected", expected.currency.clone())
                    .with_detail("actual", cost.currency.clone())),
                    None => Err(HostError::new(
                        HostErrorCode::InvalidResponse,
                        "model response omitted cost usage required by the run cost budget",
                    )),
                },
                _ => Ok(None),
            };
            let accounting = consumed.and_then(|consumed| {
                cost_settlement.and_then(|cost| {
                    pending
                        .request
                        .budget
                        .settle_usage(reservation, consumed, cost)
                })
            });
            match accounting {
                Ok(()) => Ok(response),
                Err(error) => {
                    let release = pending
                        .request
                        .budget
                        .release_usage(reservation, cost_reservation.clone());
                    Err(release.err().unwrap_or(error))
                }
            }
        }
        Err(error) => {
            let release = pending
                .request
                .budget
                .release_usage(reservation, cost_reservation);
            Err(release.err().unwrap_or(error))
        }
    };
    machine.resume_model_result(result);
    true
}

fn policy_subject(request: &etas_host::ModelRequest) -> PolicySubject {
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("model".to_owned()),
        ),
        (
            "qualified_action".to_owned(),
            HostValue::String("Model.invoke".to_owned()),
        ),
        (
            "model".to_owned(),
            HostValue::String(request.model.0.clone()),
        ),
        (
            "resource".to_owned(),
            HostValue::String(request.model.0.clone()),
        ),
        (
            "tool_count".to_owned(),
            HostValue::UInt(request.tools.len() as u128),
        ),
    ];
    if let Some(provider) = &request.provider {
        attributes.push(("provider".to_owned(), HostValue::String(provider.0.clone())));
    }
    PolicySubject {
        kind: "model".to_owned(),
        attributes,
    }
}
