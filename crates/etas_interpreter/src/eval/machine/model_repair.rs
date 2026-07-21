use etas_host::{ModelContent, ModelMessage, ModelRole, ModelToolChoice};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelRepairKind {
    RequiredToolChoice,
    TypedOutput,
}

impl ModelRepairKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequiredToolChoice => "required_tool_choice",
            Self::TypedOutput => "typed_output",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelRepairDirective {
    pub kind: ModelRepairKind,
    pub attempt: usize,
    pub reason: String,
    pub message: ModelMessage,
}

#[derive(Clone, Debug)]
pub struct ModelRepairExhausted {
    pub kind: ModelRepairKind,
    pub attempts: usize,
    pub reason: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ModelRepairPolicy {
    max_attempts: usize,
}

impl ModelRepairPolicy {
    pub fn new(max_attempts: usize) -> Self {
        Self { max_attempts }
    }

    pub fn required_tool_choice(
        self,
        attempts_used: usize,
        choice: &ModelToolChoice,
    ) -> Result<ModelRepairDirective, ModelRepairExhausted> {
        let reason =
            "model returned final content before satisfying required tool-call choice".to_owned();
        self.directive_or_exhausted(
            ModelRepairKind::RequiredToolChoice,
            attempts_used,
            reason,
            required_tool_choice_repair_message(choice),
        )
    }

    pub fn typed_output(
        self,
        attempts_used: usize,
        error: &str,
    ) -> Result<ModelRepairDirective, ModelRepairExhausted> {
        self.directive_or_exhausted(
            ModelRepairKind::TypedOutput,
            attempts_used,
            format!("typed output decoder rejected the model response: {error}"),
            typed_output_repair_message(error),
        )
    }

    fn directive_or_exhausted(
        self,
        kind: ModelRepairKind,
        attempts_used: usize,
        reason: String,
        message: ModelMessage,
    ) -> Result<ModelRepairDirective, ModelRepairExhausted> {
        if attempts_used < self.max_attempts {
            Ok(ModelRepairDirective {
                kind,
                attempt: attempts_used + 1,
                reason,
                message,
            })
        } else {
            Err(ModelRepairExhausted {
                kind,
                attempts: attempts_used,
                reason,
            })
        }
    }
}

fn typed_output_repair_message(error: &str) -> ModelMessage {
    ModelMessage {
        role: ModelRole::User,
        content: vec![ModelContent::Text(format!(
            "The previous response did not satisfy the required typed output contract. Return exactly one valid JSON value matching the requested schema. Do not include markdown, prose, escaped JSON strings, or extra keys. Decoder error: {error}"
        ))],
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

fn required_tool_choice_repair_message(choice: &ModelToolChoice) -> ModelMessage {
    let instruction = match choice {
        ModelToolChoice::RequiredTool(tool) => format!(
            "The previous response did not satisfy the required tool-call contract. You must call the `{tool}` tool before producing the final answer. Return a tool call now; do not return final JSON or prose."
        ),
        ModelToolChoice::RequiredAny => {
            "The previous response did not satisfy the required tool-call contract. You must call one available tool before producing the final answer. Return a tool call now; do not return final JSON or prose.".to_owned()
        }
        ModelToolChoice::Auto => {
            "The previous response did not satisfy the model tool-call contract.".to_owned()
        }
    };
    ModelMessage {
        role: ModelRole::User,
        content: vec![ModelContent::Text(instruction)],
        tool_call_id: None,
        tool_calls: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_tool_choice_repair_is_explicit_and_bounded() {
        let policy = ModelRepairPolicy::new(1);
        let repair = policy
            .required_tool_choice(0, &ModelToolChoice::RequiredTool("Search".to_owned()))
            .expect("first repair should be allowed");

        assert_eq!(repair.kind, ModelRepairKind::RequiredToolChoice);
        assert_eq!(repair.attempt, 1);
        assert!(repair.reason.contains("required tool-call choice"));
        let [ModelContent::Text(instruction)] = repair.message.content.as_slice() else {
            panic!("repair prompt must be text");
        };
        assert!(instruction.contains("Search"));

        let exhausted = policy
            .required_tool_choice(1, &ModelToolChoice::RequiredTool("Search".to_owned()))
            .expect_err("second repair should exceed policy");
        assert_eq!(exhausted.kind, ModelRepairKind::RequiredToolChoice);
        assert_eq!(exhausted.attempts, 1);
    }
}
