use super::project::{Projector, optional};
use crate::value::{CodeMetadataPayload as M, InterpValue as V};
use etas_code::{
    descriptor::DescriptorLimits,
    report::{CheckReport, DependencyInfo},
};
use etas_types::{TypeId, TypeStore, facts::symbols::KnownCodeTypes};

pub(in crate::eval) fn check_report(
    store: &TypeStore,
    known: &KnownCodeTypes,
    ty: TypeId,
    value: &CheckReport,
) -> Result<V, String> {
    value
        .validate(DescriptorLimits::default())
        .map_err(|error| error.to_string())?;
    let p = Projector { store, known };
    let diagnostic = p.array_element(p.field(ty, "diagnostics")?)?;
    let diagnostics = value
        .diagnostics
        .iter()
        .cloned()
        .map(|value| p.diagnostic(diagnostic, value))
        .collect::<Result<Vec<_>, _>>()?;
    let revision = p.field(ty, "revision")?;
    let effects = p.field(ty, "effects")?;
    let graph = &value.descriptors;
    let effects_value = super::behavior::effects(
        &p,
        effects,
        value.effects.declared_bound.map(|row| (graph, row)),
        Some((graph, &value.effects)),
    )?;
    let runtime = p.array_element(p.field(ty, "runtime_checks")?)?;
    let checks = value
        .runtime_checks
        .iter()
        .map(|value| {
            p.record(
                runtime,
                vec![
                    ("kind", V::String(value.kind.clone().into())),
                    (
                        "location",
                        optional(
                            value
                                .location
                                .clone()
                                .map(|value| {
                                    p.location(p.option(p.field(runtime, "location")?)?, value)
                                })
                                .transpose()?,
                        ),
                    ),
                    (
                        "spec",
                        p.optional_metadata(
                            p.field(runtime, "spec")?,
                            value.spec.clone().map(M::Definition),
                        )?,
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    p.record(
        ty,
        vec![
            ("status", V::String(value.status.as_str().into())),
            ("diagnostics", V::Array(diagnostics.into())),
            (
                "revision",
                revision_info(store, known, revision, &value.revision)?,
            ),
            (
                "dependencies",
                dependencies(&p, p.field(ty, "dependencies")?, &value.dependencies)?,
            ),
            (
                "snapshot_fingerprint",
                V::String(value.snapshot_fingerprint.to_string().into()),
            ),
            ("effects", effects_value),
            (
                "determinism",
                p.optional_metadata(
                    p.field(ty, "determinism")?,
                    value.determinism.map(M::Determinism),
                )?,
            ),
            (
                "trace",
                p.optional_metadata(
                    p.field(ty, "trace")?,
                    value.trace.map(|root| M::Trace {
                        graph: graph.clone(),
                        root,
                    }),
                )?,
            ),
            ("runtime_checks", V::Array(checks.into())),
        ],
    )
}

pub(in crate::eval) fn revision_info(
    store: &TypeStore,
    known: &KnownCodeTypes,
    ty: TypeId,
    revision: &etas_code::RevisionInfo,
) -> Result<V, String> {
    Projector { store, known }.record(
        ty,
        vec![
            (
                "key",
                V::String(revision.key().fingerprint().to_string().into()),
            ),
            (
                "content",
                V::String(revision.inputs().content.to_string().into()),
            ),
            (
                "parent",
                optional(
                    revision
                        .inputs()
                        .parent
                        .map(|parent| V::String(parent.fingerprint().to_string().into())),
                ),
            ),
        ],
    )
}

pub(super) fn dependencies(
    p: &Projector<'_>,
    ty: TypeId,
    value: &DependencyInfo,
) -> Result<V, String> {
    let module = p.array_element(p.field(ty, "modules")?)?;
    let modules = value
        .modules
        .iter()
        .map(|value| {
            p.record(
                module,
                vec![
                    (
                        "module",
                        V::Array(
                            value
                                .module
                                .iter()
                                .map(|value| V::String(value.clone().into()))
                                .collect::<Vec<_>>()
                                .into(),
                        ),
                    ),
                    (
                        "package",
                        V::String(value.package.fingerprint().to_string().into()),
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    let binding = p.array_element(p.field(ty, "bindings")?)?;
    let bindings = value
        .bindings
        .iter()
        .map(|value| {
            p.record(
                binding,
                vec![
                    (
                        "path",
                        V::Array(
                            value
                                .path
                                .iter()
                                .map(|value| V::String(value.clone().into()))
                                .collect::<Vec<_>>()
                                .into(),
                        ),
                    ),
                    (
                        "definition",
                        p.metadata(
                            p.field(binding, "definition")?,
                            M::Definition(value.definition.clone()),
                        )?,
                    ),
                ],
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    let artifact = p.array_element(p.field(ty, "artifacts")?)?;
    let artifacts = value
        .artifacts
        .iter()
        .map(|value| {
            p.record(
                artifact,
                vec![
                    ("import_root", V::String(value.import_root.clone().into())),
                    ("fingerprint", V::String(value.fingerprint.clone().into())),
                ],
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    p.record(
        ty,
        vec![
            (
                "environment",
                V::String(value.environment.to_string().into()),
            ),
            (
                "lexical_owner",
                p.metadata(
                    p.field(ty, "lexical_owner")?,
                    M::Definition(value.lexical_owner.clone()),
                )?,
            ),
            (
                "standard_package",
                V::String(value.standard_package.fingerprint().to_string().into()),
            ),
            ("modules", V::Array(modules.into())),
            ("bindings", V::Array(bindings.into())),
            ("artifacts", V::Array(artifacts.into())),
        ],
    )
}
