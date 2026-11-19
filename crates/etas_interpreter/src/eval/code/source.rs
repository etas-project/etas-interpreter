//! Source records at the compiler primitive boundary. Descriptive fields are
//! ordinary Etas values; only private retained fields carry native leases.
use etas_code::syntax::CodeSyntaxOperations;
use etas_core::Span;
use etas_types::TypeId;

use super::{definition_fields, project::Projector};
use crate::{
    control::ControlSignal,
    eval::EvalContext,
    value::{
        AgentDefinitionValue, CodeMetadataPayload as M, InterpValue as V, SyntaxBlockValue,
        source_code::Definition,
    },
};

type ResultValue = Result<V, ControlSignal>;

impl EvalContext<'_> {
    pub(in crate::eval) fn execute_source_code_primitive(
        &mut self,
        item: etas_hir::HirItemId,
        role: etas_std::StdExecutionRole,
        args: &[V],
        bindings: &crate::control::GenericBindings,
        span: Span,
    ) -> ControlSignal {
        if let Some(signal) = self.code_operation_boundary(span) {
            return signal;
        }
        let etas_types::ItemSignature::Flow(signature) = &self.checked.types.item_signatures[&item]
        else {
            unreachable!("checked compiler execution role signature")
        };
        let output = match self.materialized_type(signature.output, bindings, span) {
            Ok(ty) => ty,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        let control = match self.code_syntax_control(span) {
            Ok(control) => control,
            Err(fault) => return ControlSignal::Fault(Box::new(fault)),
        };
        if role == etas_std::StdExecutionRole::ParseBody {
            let [V::String(text)] = args else {
                return missing("parse_body requires its checked text argument", span);
            };
            let Some((syntax_ty, error_ty)) = etas_types::value_representation::result_parameters(
                &self.checked.type_store,
                &self.checked.types.known_std_types,
                output,
            ) else {
                return missing("parse_body has no checked source Result ABI", span);
            };
            let (name, result) = match self.code_syntax.parse_body_bounded_controlled(
                text,
                self.syntax_limits,
                &control,
            ) {
                Ok(syntax) => (
                    "Ok",
                    self.source_syntax_value(
                        syntax_ty,
                        SyntaxBlockValue::new(syntax_ty, syntax, self.code_syntax.clone()),
                        span,
                    ),
                ),
                Err(etas_code::syntax::ParseBodyFailure::InvalidSyntax(error)) => (
                    "Err",
                    super::diagnostic::parse_error(
                        &self.checked.type_store,
                        &self.checked.types.known_std_types.code,
                        &self.checked.types.enum_layouts,
                        error_ty,
                        error,
                    )
                    .map_err(|reason| missing(reason, span)),
                ),
                Err(etas_code::syntax::ParseBodyFailure::Operation(error)) => {
                    return self.controlled_syntax_failure(error, span);
                }
            };
            return match result {
                Ok(value) => self.checked_result_value(output, name, value, span),
                Err(signal) => signal,
            };
        }
        let result = (|| -> ResultValue {
            let [definition, syntax] = args else {
                return Err(missing(
                    "with_body requires a definition and retained syntax",
                    span,
                ));
            };
            let definition = Definition::input(definition, self.checked)
                .map_err(|reason| missing(reason, span))?;
            let syntax = crate::value::source_code::syntax_input(syntax, self.checked)
                .map_err(|reason| missing(reason, span))?;
            let expected_family = matches!(
                (&definition, role),
                (
                    Definition::Agent(_),
                    etas_std::StdExecutionRole::AgentWithBody
                ) | (
                    Definition::Callable(_),
                    etas_std::StdExecutionRole::CallableWithBody
                )
            );
            if !expected_family {
                return Err(missing(
                    "with_body received a different retained definition family",
                    span,
                ));
            }
            let reference = definition
                .reference()
                .map_err(|reason| missing(reason, span))?;
            let syntax = syntax
                .for_definition(
                    &reference.revision().inputs().definition,
                    definition.backend().syntax_operations(),
                    self.syntax_limits,
                    &control,
                )
                .map_err(|error| self.controlled_syntax_failure(error, span))?;
            let edited = definition
                .backend()
                .with_body_controlled(reference, syntax, &control)
                .map_err(|error| match error {
                    etas_code::definition::DefinitionOperationError::Syntax(error) => {
                        self.controlled_syntax_failure(error, span)
                    }
                    error => super::definition_operation_failure(error.into(), span),
                })?;
            let definition = match definition {
                Definition::Agent(value) => Definition::Agent(AgentDefinitionValue::with_abi(
                    value.ty(),
                    edited,
                    value.backend().clone(),
                    self.checked.clone(),
                )),
                Definition::Callable(value) => Definition::Callable(
                    value
                        .with_definition(edited)
                        .map_err(|reason| missing(reason, span))?,
                ),
            };
            self.source_definition_value(output, definition, span)
        })();
        match result {
            Ok(value) => self
                .code_operation_boundary(span)
                .unwrap_or(ControlSignal::Value(value)),
            Err(signal) => signal,
        }
    }

    pub(in crate::eval) fn source_compiler_value(&self, value: V, span: Span) -> ControlSignal {
        let result = if !self.checked.hir.source_standard_types {
            Ok(value)
        } else {
            match value {
                V::AgentDefinition(value) => {
                    self.source_definition_value(value.ty(), Definition::Agent(value), span)
                }
                V::CallableDefinition(value) => {
                    self.source_definition_value(value.ty(), Definition::Callable(value), span)
                }
                V::SyntaxBlock(value) => self.source_syntax_value(value.ty(), value, span),
                V::CodeMetadata(value) => self
                    .source_projector()
                    .metadata(value.ty(), value.payload().clone())
                    .map_err(|reason| missing(reason, span)),
                value => Ok(value),
            }
        };
        match result {
            Ok(value) => self
                .code_operation_boundary(span)
                .unwrap_or(ControlSignal::Value(value)),
            Err(signal) => signal,
        }
    }

    fn source_projector(&self) -> Projector<'_> {
        Projector {
            store: &self.checked.type_store,
            known: &self.checked.types.known_std_types.code,
            enums: &self.checked.types.enum_layouts,
        }
    }

    pub(super) fn source_syntax_value(
        &self,
        ty: TypeId,
        value: SyntaxBlockValue,
        span: Span,
    ) -> ResultValue {
        let p = self.source_projector();
        let control = self
            .code_syntax_control(span)
            .map_err(|fault| ControlSignal::Fault(Box::new(fault)))?;
        let retained = p
            .field(ty, "retained")
            .map_err(|reason| missing(reason, span))?;
        let token_ty = p
            .field(ty, "tokens")
            .and_then(|ty| p.array_element(ty))
            .map_err(|reason| missing(reason, span))?;
        let kind_ty = p
            .field(token_ty, "kind")
            .map_err(|reason| missing(reason, span))?;
        let text = value
            .format_controlled(self.syntax_limits.max_formatted_bytes, &control)
            .map_err(|error| self.controlled_syntax_failure(error, span))?;
        let tokens = value
            .backend()
            .observe_tokens_controlled(value.syntax(), self.syntax_limits, &control)
            .map_err(|error| self.controlled_syntax_failure(error, span))?;
        let tokens = tokens
            .into_iter()
            .map(|token| {
                let kind = p.variant(kind_ty, token.kind, Vec::new())?;
                p.record(
                    token_ty,
                    vec![("kind", kind), ("text", V::String(token.text.into()))],
                )
            })
            .collect::<Result<Vec<_>, String>>()
            .map_err(|reason| missing(reason, span))?;
        let maximum = self
            .syntax_limits
            .max_formatted_bytes
            .min(value.backend().max_formatted_bytes());
        p.record(
            ty,
            vec![
                ("retained", V::SyntaxBlock(value.in_revision(retained))),
                ("text", V::String(text.into())),
                ("tokens", V::Array(tokens.into())),
                ("format_limit", V::usize(maximum)),
            ],
        )
        .map_err(|reason| missing(reason, span))
    }

    fn source_definition_value(&self, ty: TypeId, value: Definition, span: Span) -> ResultValue {
        let p = self.source_projector();
        let fields = p.fields(ty).map_err(|reason| missing(reason, span))?;
        let mut output = Vec::with_capacity(fields.fields.len());
        for field in fields.fields {
            if let Some(signal) = self.code_operation_boundary(span) {
                return Err(signal);
            }
            let payload = match field.name.as_str() {
                "retained" => match &value {
                    Definition::Agent(value) => V::AgentDefinition(AgentDefinitionValue::with_abi(
                        field.ty,
                        value.definition().clone(),
                        value.backend().clone(),
                        self.checked.clone(),
                    )),
                    Definition::Callable(value) => {
                        V::CallableDefinition(value.in_revision(field.ty, self.checked.clone()))
                    }
                },
                "observation" => self.definition_observation(&p, field.ty, &value, span)?,
                "body" => {
                    let body = match &value {
                        Definition::Agent(value) => value
                            .body()
                            .map_err(|error| missing(error.to_string(), span))?,
                        Definition::Callable(value) => value.body().cloned(),
                    };
                    let syntax_ty = p.option(field.ty).map_err(|reason| missing(reason, span))?;
                    let body = body
                        .map(|body| {
                            self.source_syntax_value(
                                syntax_ty,
                                SyntaxBlockValue::new(
                                    syntax_ty,
                                    body,
                                    value.backend().syntax_operations().clone(),
                                ),
                                span,
                            )
                        })
                        .transpose()?;
                    p.optional_value(field.ty, body)
                        .map_err(|reason| missing(reason, span))?
                }
                name => {
                    let payload = match &value {
                        Definition::Agent(value) => definition_fields::agent_field(
                            p.store, p.known, p.enums, field.ty, value, name,
                        ),
                        Definition::Callable(value) => definition_fields::callable_field(
                            p.store, p.known, p.enums, field.ty, value, name,
                        ),
                    }
                    .map_err(|reason| missing(reason, span))?;
                    if name == "methods" {
                        let element = p
                            .array_element(field.ty)
                            .map_err(|reason| missing(reason, span))?;
                        let V::Array(methods) = payload else {
                            return Err(missing(
                                "compiler methods have no array representation",
                                span,
                            ));
                        };
                        let mut output = Vec::new();
                        for method in methods.borrow().iter() {
                            let V::CallableDefinition(method) = method else {
                                return Err(missing(
                                    "compiler method has no native observation",
                                    span,
                                ));
                            };
                            output.push(self.source_definition_value(
                                element,
                                Definition::Callable(method.clone()),
                                span,
                            )?);
                        }
                        V::Array(output.into())
                    } else {
                        payload
                    }
                }
            };
            output.push((field.name, payload));
        }
        p.record(
            ty,
            output
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone()))
                .collect(),
        )
        .map_err(|reason| missing(reason, span))
    }

    fn definition_observation(
        &self,
        p: &Projector<'_>,
        ty: TypeId,
        value: &Definition,
        span: Span,
    ) -> ResultValue {
        let control = self
            .code_syntax_control(span)
            .map_err(|fault| ControlSignal::Fault(Box::new(fault)))?;
        if let Ok(reference) = value.reference() {
            let view = value
                .backend()
                .resolve(reference)
                .map_err(|error| missing(error.to_string(), span))?;
            let snapshot = view
                .diff_snapshot(
                    value.backend().syntax_operations().as_ref(),
                    self.syntax_limits.max_formatted_bytes,
                    &control,
                )
                .map_err(|error| super::definition_operation_failure(error, span))?;
            return observation(p, ty, &snapshot).map_err(|reason| missing(reason, span));
        }
        let Definition::Callable(value) = value else {
            unreachable!("agent definitions always retain a reference")
        };
        // Read-only method observations retain their owning method snapshot,
        // without acquiring an editable DefinitionRef or compilation authority.
        let header = value.header();
        let identity = header
            .id
            .as_declaration()
            .or_else(|| {
                header
                    .id
                    .as_method()
                    .map(|method| method.implementation().subject())
            })
            .ok_or_else(|| missing("callable observation has no canonical owner", span))?;
        let graph = value.descriptors();
        let Some(etas_code::descriptor::TypeInfo::Function(signature)) =
            graph.type_info(header.signature)
        else {
            return Err(missing(
                "callable observation has no signature descriptor",
                span,
            ));
        };
        let row = |row| {
            graph
                .row_fingerprint(row)
                .map_err(|error| missing(error.to_string(), span))
        };
        let current = value.analysis();
        let current_row = |select: fn(
            &etas_code::descriptor::CallableAnalysisInfo,
        )
            -> Option<etas_code::descriptor::EffectRowInfoRef>| {
            current
                .and_then(|(graph, value)| select(value).map(|row| graph.row_fingerprint(row)))
                .transpose()
                .map_err(|error| missing(error.to_string(), span))
        };
        let body = value
            .body()
            .map(|body| {
                value.syntax_operations().format_bounded_controlled(
                    body,
                    self.syntax_limits.max_formatted_bytes,
                    &control,
                )
            })
            .transpose()
            .map_err(|error| self.controlled_syntax_failure(error, span))?
            .map(|body| {
                etas_code::CodeFingerprint::of_bytes("etas.code.diff-body.v1", body.as_bytes())
            });
        let revision = value.revision().map_err(|reason| missing(reason, span))?;
        let snapshot = etas_code::definition::DefinitionDiffSnapshot {
            identity,
            revision: &revision,
            signature: Some(
                graph
                    .type_fingerprint(header.signature)
                    .map_err(|error| missing(error.to_string(), span))?,
            ),
            body,
            declared_bound: signature.effects.map(row).transpose()?,
            escaping: current_row(|value| value.effects.escaping)?,
            requested: current_row(|value| value.effects.requested)?,
            default_handled: current_row(|value| value.effects.default_handled)?,
            determinism: current.and_then(|(_, value)| value.determinism),
            trace: current
                .and_then(|(graph, value)| value.trace.map(|trace| graph.trace_fingerprint(trace)))
                .transpose()
                .map_err(|error| missing(error.to_string(), span))?,
        };
        observation(p, ty, &snapshot).map_err(|reason| missing(reason, span))
    }
}

fn observation(
    p: &Projector<'_>,
    ty: TypeId,
    value: &etas_code::definition::DefinitionDiffSnapshot<'_>,
) -> Result<V, String> {
    let optional = |name: &str, value: Option<etas_code::CodeFingerprint>| {
        p.optional_value(
            p.field(ty, name)?,
            value.map(|value| V::String(value.to_string().into())),
        )
    };
    let determinism_ty = p.field(ty, "determinism")?;
    let determinism = value
        .determinism
        .map(|value| p.metadata(p.option(determinism_ty)?, M::Determinism(value)))
        .transpose()?;
    p.record(
        ty,
        vec![
            (
                "identity",
                p.metadata(
                    p.field(ty, "identity")?,
                    M::Definition(value.identity.clone()),
                )?,
            ),
            (
                "entry",
                V::String(
                    value
                        .revision
                        .inputs()
                        .definition
                        .fingerprint()
                        .to_string()
                        .into(),
                ),
            ),
            (
                "environment",
                V::String(value.revision.inputs().environment.to_string().into()),
            ),
            ("signature", optional("signature", value.signature)?),
            ("body", optional("body", value.body)?),
            (
                "declared_bound",
                optional("declared_bound", value.declared_bound)?,
            ),
            ("escaping", optional("escaping", value.escaping)?),
            ("requested", optional("requested", value.requested)?),
            (
                "default_handled",
                optional("default_handled", value.default_handled)?,
            ),
            (
                "determinism",
                p.optional_value(determinism_ty, determinism)?,
            ),
            ("trace", optional("trace", value.trace)?),
        ],
    )
}

fn missing(reason: impl Into<String>, span: Span) -> ControlSignal {
    ControlSignal::missing_checked_fact(reason.into(), span)
}
