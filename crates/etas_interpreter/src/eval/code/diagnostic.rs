use super::project::{Projector, optional};
use crate::value::{ArrayValue, CodeMetadataPayload as M, InterpValue as V};
use etas_code::{
    diagnostic::{DiagnosticDetails, DiagnosticLocation, DiagnosticRecord, RelatedLocation},
    syntax::ParseError,
};
use etas_types::{TypeId, TypeStore, facts::symbols::KnownCodeTypes};

pub(super) fn parse_error(
    store: &TypeStore,
    known: &KnownCodeTypes,
    ty: TypeId,
    error: ParseError,
) -> Result<V, String> {
    let project = Projector { store, known };
    let item = project.array_element(project.field(ty, "diagnostics")?)?;
    let diagnostics = error
        .diagnostics
        .into_iter()
        .map(|value| project.diagnostic(item, value))
        .collect::<Result<Vec<_>, _>>()?;
    project.record(
        ty,
        vec![("diagnostics", V::Array(ArrayValue::new(diagnostics)))],
    )
}

impl Projector<'_> {
    pub fn location(&self, ty: TypeId, value: DiagnosticLocation) -> Result<V, String> {
        self.record(
            ty,
            vec![
                ("source", V::String(value.source.to_string().into())),
                ("revision", V::String(value.revision.to_string().into())),
                ("byte_start", V::usize(value.byte_start)),
                ("byte_end", V::usize(value.byte_end)),
            ],
        )
    }
    fn related(&self, ty: TypeId, value: RelatedLocation) -> Result<V, String> {
        self.record(
            ty,
            vec![
                (
                    "location",
                    self.location(self.field(ty, "location")?, value.location)?,
                ),
                ("message", V::String(value.message.into())),
            ],
        )
    }
    fn details(&self, ty: TypeId, value: &DiagnosticDetails) -> Result<V, String> {
        let item = self.array_element(self.field(ty, "related")?)?;
        let related = value
            .related
            .iter()
            .cloned()
            .map(|value| self.related(item, value))
            .collect::<Result<Vec<_>, _>>()?;
        let constraint_type = self.option(self.field(ty, "constraint")?)?;
        let constraint = value
            .constraint
            .as_ref()
            .map(|value| {
                self.record(
                    constraint_type,
                    vec![
                        ("kind", V::String(value.kind.clone().into())),
                        (
                            "owner",
                            self.optional_metadata(
                                self.field(constraint_type, "owner")?,
                                value.owner.clone().map(M::Definition),
                            )?,
                        ),
                        ("message", V::String(value.message.clone().into())),
                    ],
                )
            })
            .transpose()?;
        let step = self.array_element(self.field(ty, "analysis_path")?)?;
        let path = value
            .analysis_path
            .iter()
            .map(|value| {
                self.record(
                    step,
                    vec![
                        (
                            "callable",
                            self.metadata(
                                self.field(step, "callable")?,
                                M::Callable(value.callable.clone()),
                            )?,
                        ),
                        (
                            "location",
                            optional(
                                value
                                    .location
                                    .clone()
                                    .map(|value| {
                                        self.location(
                                            self.option(self.field(step, "location")?)?,
                                            value,
                                        )
                                    })
                                    .transpose()?,
                            ),
                        ),
                        ("message", V::String(value.message.clone().into())),
                    ],
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.record(
            ty,
            vec![
                (
                    "expected",
                    self.optional_metadata(
                        self.field(ty, "expected")?,
                        value.expected.clone().map(M::Detail),
                    )?,
                ),
                (
                    "actual",
                    self.optional_metadata(
                        self.field(ty, "actual")?,
                        value.actual.clone().map(M::Detail),
                    )?,
                ),
                ("constraint", optional(constraint)),
                ("related", V::Array(related.into())),
                ("analysis_path", V::Array(path.into())),
                (
                    "trace_witness",
                    self.optional_metadata(
                        self.field(ty, "trace_witness")?,
                        value.trace_witness.clone().map(M::TraceWitness),
                    )?,
                ),
            ],
        )
    }
    pub fn diagnostic(&self, ty: TypeId, value: DiagnosticRecord) -> Result<V, String> {
        value
            .validate(Default::default())
            .map_err(|error| error.to_string())?;
        let location = self.option(self.field(ty, "location")?)?;
        let details = self.option(self.field(ty, "details")?)?;
        self.record(
            ty,
            vec![
                ("code", V::String(value.code.into())),
                ("severity", V::String(value.severity.as_str().into())),
                ("message", V::String(value.message.into())),
                (
                    "location",
                    optional(
                        value
                            .location
                            .map(|value| self.location(location, value))
                            .transpose()?,
                    ),
                ),
                (
                    "details",
                    optional(
                        value
                            .details
                            .as_deref()
                            .map(|value| self.details(details, value))
                            .transpose()?,
                    ),
                ),
            ],
        )
    }
}
