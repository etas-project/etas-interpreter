use super::{RevisionValueFailure as Failure, Walk};
use crate::value::{AgentDefinitionValue, CodeMetadataValue, InterpValue};
use etas_types::{Type, TypeId};

impl Walk<'_> {
    pub(super) fn code_value(
        &self,
        from: TypeId,
        to: TypeId,
        value: InterpValue,
    ) -> Result<InterpValue, Failure> {
        let source = &self.source.types.known_std_types.code;
        let target = &self.target.types.known_std_types.code;
        let paired = |left, right| left == Some(from) && right == Some(to);
        match value {
            InterpValue::AgentDefinition(value) => {
                let applied = |program: &etas_frontend::CheckedProgram, ty, known| matches!(program.type_store.get(ty), Some(Type::Applied { constructor, args }) if Some(TypeId(constructor.0)) == known && args.len() == 1);
                if value.ty() != from
                    || !applied(self.source, from, source.agent_definition)
                    || !applied(self.target, to, target.agent_definition)
                {
                    return Err(Failure::UnsupportedType);
                }
                if !std::sync::Arc::ptr_eq(value.program(), self.source) {
                    return Err(Failure::WrongProgram);
                }
                Ok(InterpValue::AgentDefinition(
                    AgentDefinitionValue::with_abi(
                        to,
                        value.definition().clone(),
                        value.backend().clone(),
                        self.target.clone(),
                    ),
                ))
            }
            InterpValue::CallableDefinition(value)
                if value.ty() == from
                    && paired(source.callable_definition, target.callable_definition) =>
            {
                if !std::sync::Arc::ptr_eq(value.program(), self.source) {
                    return Err(Failure::WrongProgram);
                }
                Ok(InterpValue::CallableDefinition(
                    value.in_revision(to, self.target.clone()),
                ))
            }
            InterpValue::SyntaxBlock(value)
                if value.ty() == from && paired(source.syntax_block, target.syntax_block) =>
            {
                Ok(InterpValue::SyntaxBlock(value.in_revision(to)))
            }
            InterpValue::CodeMetadata(value) if value.ty() == from => {
                CodeMetadataValue::new(target, to, value.payload().clone())
                    .map(InterpValue::CodeMetadata)
                    .map_err(|_| Failure::InconsistentLayout)
            }
            _ => Err(Failure::UnsupportedType),
        }
    }
}
