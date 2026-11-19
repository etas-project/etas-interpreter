//! Message execution observations and the workflow event sink. Message
//! construction, identifiers and payload policy are implemented in Etas.
use super::*;
use etas_std::StdExecutionRole as R;

impl EvalContext<'_> {
    pub(super) fn execute_message_context(
        &mut self,
        item: HirItemId,
        role: R,
        args: &[InterpValue],
        span: Span,
    ) -> ControlSignal {
        if let Some(signal) = self.cancellation_signal(span) {
            return signal;
        }
        if let Err(error) = self.host_context.budget.check_time() {
            return ControlSignal::runtime_fault(error.message, span);
        }
        if role == R::MessageContextRecord {
            let [_, message] = args else {
                return ControlSignal::missing_checked_fact(
                    "message event sink requires its context and message",
                    span,
                );
            };
            return match message_event(message) {
                Ok(event) => {
                    self.events.push(event);
                    ControlSignal::Value(InterpValue::Unit)
                }
                Err(reason) => ControlSignal::missing_checked_fact(reason, span),
            };
        }
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!("checked message context signature")
        };
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(error) => {
                return ControlSignal::runtime_fault(
                    format!("runtime clock cannot capture message context: {error}"),
                    span,
                );
            }
        };
        let value = HostValue::Record(vec![
            (
                "sequence".into(),
                HostValue::UInt(self.next_message as u128),
            ),
            (
                "session".into(),
                host_option(self.current_session.clone().map(HostValue::String)),
            ),
            (
                "created_at".into(),
                HostValue::Record(vec![(
                    "unix_millis".into(),
                    HostValue::UInt(now.as_millis()),
                )]),
            ),
            (
                "trace".into(),
                host_option(Some(HostValue::String(format!(
                    "{:?}",
                    self.host_context.trace.trace_id
                )))),
            ),
        ]);
        match super::host_value::decode_checked_host_value(value, signature.output, self.checked) {
            Ok(value) => {
                self.next_message += 1;
                ControlSignal::Value(value)
            }
            Err(reason) => ControlSignal::missing_checked_fact(reason, span),
        }
    }
}

fn host_option(value: Option<HostValue>) -> HostValue {
    let (name, fields) = match value {
        Some(value) => ("Some", vec![value]),
        None => ("None", Vec::new()),
    };
    HostValue::Variant {
        name: name.into(),
        fields,
    }
}

fn message_event(message: &InterpValue) -> Result<WorkflowEvent, String> {
    let message = source_message_value(message)?;
    use crate::value::MessageRoleValue as Role;
    let role = match message.role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    };
    Ok(WorkflowEvent::MessageCreated {
        id: message.id,
        from: message.from,
        to: message.to,
        session: message.session,
        role: role.into(),
        created_at: message.created_at,
        payload: Box::new(message.payload.as_ref().clone()),
        provenance: message.provenance,
    })
}

pub(super) fn source_message_value(
    message: &InterpValue,
) -> Result<crate::value::MessageValue, String> {
    use crate::value::MessageRoleValue as Role;
    let field = |name| field(message, name);
    let role = match representation(field("role")?) {
        InterpValue::Variant { name, fields } if fields.is_empty() => match name.as_ref() {
            "System" => Role::System,
            "User" => Role::User,
            "Assistant" => Role::Assistant,
            "Tool" => Role::Tool,
            _ => return Err("message event has an unknown checked Role".into()),
        },
        _ => return Err("message event requires a checked source Role".into()),
    };
    let millis = match representation(self::field(field("created_at")?, "unix_millis")?) {
        InterpValue::Number(number) => number
            .as_u128()
            .ok_or("message clock requires unix milliseconds")?,
        _ => return Err("message clock requires unix milliseconds".into()),
    };
    let provenance = option(field("provenance")?)?
        .map(|value| {
            Ok::<_, String>(crate::value::ProvenanceValue {
                trace_id: optional_text(self::field(value, "trace_id")?)?,
                source: optional_text(self::field(value, "source")?)?,
            })
        })
        .transpose()?;
    Ok(crate::value::MessageValue {
        id: text(field("id")?)?,
        from: optional_text(field("from")?)?,
        to: None,
        session: optional_text(field("session")?)?,
        role,
        created_at: (millis / 1_000).to_string(),
        payload: field("content")?.clone().into(),
        provenance,
    })
}

fn representation(mut value: &InterpValue) -> &InterpValue {
    while let InterpValue::Nominal { value: inner, .. } = value {
        value = inner;
    }
    value
}

fn field<'a>(value: &'a InterpValue, name: &str) -> Result<&'a InterpValue, String> {
    match representation(value) {
        InterpValue::Record(record) => record
            .get_ref(name)
            .ok_or_else(|| format!("message execution record has no checked field `{name}`")),
        _ => Err("message execution requires its checked source record".into()),
    }
}

fn text(value: &InterpValue) -> Result<String, String> {
    match representation(value) {
        InterpValue::String(value) => Ok(value.to_string()),
        _ => Err("message execution requires a checked string identity".into()),
    }
}

fn option(value: &InterpValue) -> Result<Option<&InterpValue>, String> {
    match representation(value) {
        InterpValue::Variant { name, fields } if name.as_ref() == "None" && fields.is_empty() => {
            Ok(None)
        }
        InterpValue::Variant { name, fields } if name.as_ref() == "Some" && fields.len() == 1 => {
            Ok(Some(&fields[0]))
        }
        InterpValue::OptionNone => Ok(None),
        InterpValue::OptionSome(value) => Ok(Some(value)),
        _ => Err("message execution requires a checked Option value".into()),
    }
}

fn optional_text(value: &InterpValue) -> Result<Option<String>, String> {
    option(value)?.map(text).transpose()
}
