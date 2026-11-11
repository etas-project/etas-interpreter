use etas_host::{
    CostBudget, CostReservation, HostError, HostErrorCode, HostRequestKind, HostTraceRequest,
    HostValue, ModelResponse, PolicySubject, TokenReservation,
};

use crate::{
    eval::{EvalContext, machine::EvalMachine},
    host::HostServices,
};

use super::{
    host_dispatch::{HostCompletion, HostDispatch, PendingHostOperation},
    policy::{boundary_policy_ref_for, evaluate_before_boundary},
};

type ModelCall<'host> = crate::host::HostFuture<'host, Result<ModelResponse, etas_host::HostError>>;

pub(in crate::driver) enum ModelDispatchStart<'host> {
    Complete(bool),
    Pending(Box<PendingModelHostDispatch<'host>>),
}

struct ModelAccounting {
    pending: crate::control::PendingModel,
    remaining_tokens: Option<u64>,
    remaining_cost: Option<CostBudget>,
    token_reservation: TokenReservation,
    cost_reservation: Option<CostReservation>,
}

pub(in crate::driver) struct PendingModelHostDispatch<'host> {
    accounting: ModelAccounting,
    pending: PendingHostOperation<ModelResponse, ModelCall<'host>>,
}

pub(in crate::driver) struct CompletedModelHostDispatch {
    accounting: ModelAccounting,
    completion: HostCompletion<ModelResponse>,
}

pub(in crate::driver) async fn begin<'host>(
    eval: &mut EvalContext<'_>,
    host: &'host dyn HostServices,
    pending: crate::control::PendingModel,
    machine: &mut EvalMachine,
) -> ModelDispatchStart<'host> {
    let policy_ref = boundary_policy_ref_for(eval, pending.request.policy_ref.clone());
    let trace_subject = policy_subject(&pending.request);
    if !evaluate_before_boundary(
        eval,
        host,
        policy_ref,
        trace_subject.clone(),
        pending.span,
        "model",
    )
    .await
    {
        return ModelDispatchStart::Complete(false);
    }

    if let Err(error) = pending.request.budget.check_time() {
        machine.resume_model_result(Err(error));
        return ModelDispatchStart::Complete(true);
    }
    let remaining_tokens = match pending.request.budget.remaining_tokens() {
        Ok(remaining) => remaining,
        Err(error) => {
            machine.resume_model_result(Err(error));
            return ModelDispatchStart::Complete(true);
        }
    };
    if remaining_tokens == Some(0) {
        machine.resume_model_result(Err(HostError::new(
            HostErrorCode::BudgetExceeded,
            "run-owned execution token budget is exhausted",
        )));
        return ModelDispatchStart::Complete(true);
    }
    let reservation_amount = remaining_tokens
        .or(pending.request.options.max_output_tokens)
        .unwrap_or(0);
    let reservation = match pending.request.budget.reserve_tokens(reservation_amount) {
        Ok(reservation) => reservation,
        Err(error) => {
            machine.resume_model_result(Err(error));
            return ModelDispatchStart::Complete(true);
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
            return ModelDispatchStart::Complete(true);
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
                return ModelDispatchStart::Complete(true);
            }
        },
        None => None,
    };

    let request_id = pending.request.id;
    let trace_payload = pending.request.trace_payload();
    match HostDispatch::begin(
        eval,
        request_id,
        HostRequestKind::Model,
        trace_payload,
        pending.request.authority.clone(),
        pending.request.trace.clone(),
        |operation| host.model(operation, pending.request.clone()),
    ) {
        Ok(host_pending) => ModelDispatchStart::Pending(Box::new(PendingModelHostDispatch {
            accounting: ModelAccounting {
                pending,
                remaining_tokens,
                remaining_cost,
                token_reservation: reservation,
                cost_reservation,
            },
            pending: host_pending,
        })),
        Err(error) => {
            let release = pending
                .request
                .budget
                .release_usage(reservation, cost_reservation);
            machine.resume_model_result(Err(release.err().unwrap_or(error)));
            ModelDispatchStart::Complete(true)
        }
    }
}

pub(in crate::driver) fn finish(
    eval: &mut EvalContext<'_>,
    machine: &mut EvalMachine,
    completed: CompletedModelHostDispatch,
) {
    let ModelAccounting {
        pending,
        remaining_tokens,
        remaining_cost,
        token_reservation: reservation,
        cost_reservation,
    } = completed.accounting;
    let result = match HostDispatch::finish(eval, completed.completion) {
        Ok(response) => {
            let consumed = match response
                .usage
                .as_ref()
                .and_then(|usage| usage.input_tokens.zip(usage.output_tokens))
            {
                Some((input, output)) => input.checked_add(output).map(Some).ok_or_else(|| {
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
                None => Ok(None),
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
                cost_settlement.and_then(|cost| match consumed {
                    Some(consumed) => {
                        pending
                            .request
                            .budget
                            .settle_usage(reservation, consumed, cost)
                    }
                    None => pending
                        .request
                        .budget
                        .release_usage(reservation, cost_reservation.clone()),
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
}

impl PendingModelHostDispatch<'_> {
    pub(in crate::driver) fn operation_id(&self) -> etas_host::execution::OperationId {
        self.pending.operation_id()
    }

    pub(in crate::driver) async fn poll(self) -> CompletedModelHostDispatch {
        CompletedModelHostDispatch {
            accounting: self.accounting,
            completion: HostDispatch::poll(self.pending).await,
        }
    }
}

fn policy_subject(request: &etas_host::ModelRequest) -> PolicySubject {
    let mut attributes = vec![
        (
            "action_kind".to_owned(),
            HostValue::String("model".to_owned()),
        ),
        (
            "qualified_action".to_owned(),
            HostValue::String(request.action.qualified_name().to_owned()),
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
