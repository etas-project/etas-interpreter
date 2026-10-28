use super::*;

#[tokio::test(flavor = "current_thread")]
async fn compiler_specs_keep_named_identity_and_inline_typed_constraints_without_authority() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = [
        "module api; public spec Stage<I, O>: callable I => O ![]; public spec Quiet: trace = -Console.stdout_write;",
        r#"module app;
import api.Stage as Renamed;
import api.Quiet;
type Hidden = { private_spec_catalog_field: string };
flow unrelated(value: Hidden) -> Hidden { return value; }
agent Worker(value: i32) -> i32 ~ Renamed<i32, i32> + Quiet + (-Console.stdout_write | -Console) { return value; }
flow main() -> Array<std.compiler.SpecInfo> { return (^Worker).specs; }
"#,
    ].into_iter().enumerate().map(|(id, text)| SourceInput {
        id: etas_core::SourceId(90 + id as u32), path: None, text: text.into(), kind: etas_frontend::SourceKind::SourceProjectFile,
    }).collect();
    let (checked, definitions) = prepared_project(input, &[]);
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Array(specs)) = result.value() else {
        panic!("specs");
    };
    let specs = specs.borrow();
    assert_eq!(specs.len(), 3);
    let named = spec_record(&specs[0]);
    let Some(InterpValue::OptionSome(value)) = named.get("definition") else {
        panic!("definition option");
    };
    let InterpValue::CodeMetadata(value) = value.as_ref() else {
        panic!("definition");
    };
    let crate::value::CodeMetadataPayload::Definition(identity) = value.payload() else {
        panic!("definition role");
    };
    assert_eq!(identity.module(), ["api"]);
    assert_eq!(identity.declaration(), ["Stage"]);
    let Some(InterpValue::Array(arguments)) = named.get("arguments") else {
        panic!("arguments");
    };
    assert_eq!(arguments.borrow().len(), 2);
    let inline = spec_record(&specs[2]);
    assert!(matches!(
        inline.get("definition"),
        Some(InterpValue::OptionNone)
    ));
    let Some(InterpValue::OptionSome(value)) = inline.get("trace") else {
        panic!("trace option");
    };
    let InterpValue::CodeMetadata(value) = value.as_ref() else {
        panic!("trace constraint");
    };
    let crate::value::CodeMetadataPayload::TraceConstraint { graph, value } = value.payload()
    else {
        panic!("trace constraint role");
    };
    assert_eq!(value.alternatives.len(), 2);
    use etas_code::descriptor::{EffectTargetInfo, TraceRuleInfo};
    assert!(graph.data().types.iter().any(|ty| matches!(ty, etas_code::descriptor::TypeInfo::Record(fields) if fields.iter().any(|field| field.name == "private_spec_catalog_field"))));
    let TraceRuleInfo::Deny(pattern) = &value.alternatives[0][0] else {
        panic!("deny");
    };
    assert!(
        matches!(&graph.effect_row(pattern.row).unwrap().effects[0].target, EffectTargetInfo::Action { effect, name } if effect.declaration() == ["Console"] && name == "stdout_write")
    );
    let exported = crate::api::codec::value_json(result.value().unwrap());
    assert!(exported.to_string().contains("TraceConstraint"));
    assert!(!exported.to_string().contains("private_spec_catalog_field"));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_specs_survive_drafts_and_bare_marker_agents_have_no_invented_entry() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.compiler.parse_body;
spec Quiet: trace = -Console.stdout_write;
spec Marker;
agent Worker() -> i32 ~ Quiet + (-Console.stdout_write) { return 1; }
agent Bare;
impl agent Bare ~ Marker {}
flow main() -> bool {
    let original = ^Worker;
    let body = match parse_body("return 2;") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    let marker = match (^Bare).specs.get(0) { Some(spec) => spec, None => { return false; }, };
    return original.specs == draft.specs && original.specs.len() == 2
        && marker.definition != None && marker.trace == None && (^Bare).signature == None;
}
"#,
        &["app.Worker"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

fn spec_record(value: &InterpValue) -> &crate::value::RecordValue {
    let InterpValue::Nominal { value, .. } = value else {
        panic!("SpecInfo nominal");
    };
    let InterpValue::Record(value) = value.as_ref() else {
        panic!("SpecInfo record");
    };
    value
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_protocol_constraints_keep_their_distinct_declaration_kind() {
    let (checked, definitions) = prepared(
        r#"
module app;
protocol Review {}
agent Worker() -> unit ~ Review { return; }
flow main() -> Array<std.compiler.SpecInfo> { return (^Worker).specs; }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Array(specs)) = result.value() else {
        panic!("specs");
    };
    let specs = specs.borrow();
    assert_eq!(specs.len(), 1);
    let record = spec_record(&specs[0]);
    let Some(InterpValue::OptionSome(value)) = record.get("definition") else {
        panic!("protocol identity");
    };
    let InterpValue::CodeMetadata(value) = value.as_ref() else {
        panic!("definition metadata");
    };
    assert!(
        matches!(value.payload(), crate::value::CodeMetadataPayload::Definition(key) if key.kind() == etas_code::DefinitionKind::Protocol && key.declaration() == ["Review"])
    );
    assert_eq!(record.get("trace"), Some(InterpValue::OptionNone));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_annotations_preserve_static_structures_and_registered_constructors() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.runtime.limits.Tokens;
let label = "shared";
@model("annotated-model", adapter = "mock-provider")
@limits([Tokens(31)])
@trace(VirtualStages([Logical]))
@custom(details = { enabled = true, count = 7, label }, labels = ["a", "b"], ratio = 0.5, letter = 'x')
agent Worker() -> i32 { return 1; }
flow main() -> Array<std.compiler.AnnotationInfo> { return (^Worker).annotations; }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Array(annotations)) = result.value() else {
        panic!("missing annotations")
    };
    let annotations = annotations.borrow();
    assert_eq!(annotations.len(), 4);
    use etas_code::descriptor::{
        AnnotationConstructorKind as K, AnnotationLiteralInfo as L, StaticValueInfo as S,
    };
    assert!(
        matches!(static_argument(&annotations[0], 0).as_ref(), S::Literal(L::String(value)) if value == "annotated-model")
    );
    let limit = static_argument(&annotations[1], 0);
    let S::Sequence { elements, .. } = limit.as_ref() else {
        panic!("missing limit list")
    };
    let S::Constructor {
        kind: K::Limit,
        target,
        arguments,
    } = elements[0].as_ref()
    else {
        panic!("missing checked limit constructor")
    };
    assert!(
        matches!(target, etas_code::descriptor::AnnotationReferenceInfo::Declaration(key) if key.kind() == etas_code::DefinitionKind::Requirement && key.module() == ["std", "runtime", "limits"] && key.declaration() == ["Tokens"])
    );
    assert!(matches!(arguments[0].value.as_ref(), S::Literal(L::Integer(value)) if value == "31"));
    assert!(matches!(
        static_argument(&annotations[2], 0).as_ref(),
        S::Constructor { kind: K::Trace, .. }
    ));
    let record = static_argument(&annotations[3], 0);
    let S::Record(fields) = record.as_ref() else {
        panic!("missing static record")
    };
    assert_eq!(fields[0].name, "enabled");
    assert!(
        matches!(fields[2].value.as_ref(), S::Reference(etas_code::descriptor::AnnotationReferenceInfo::Declaration(key)) if key.declaration() == ["label"])
    );
    assert!(matches!(
        fields[0].value.as_ref(),
        S::Literal(L::Bool(true))
    ));
    assert!(
        matches!(static_argument(&annotations[3], 2).as_ref(), S::Literal(L::Float(value)) if value == "0.5")
    );
    assert!(matches!(
        static_argument(&annotations[3], 3).as_ref(),
        S::Literal(L::Character('x'))
    ));
    let json = crate::api::codec::value_json(result.value().unwrap()).to_string();
    assert!(json.contains("annotated-model") && json.contains("StaticValue"));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_annotations_keep_canonical_enum_and_const_references_across_aliases() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = [
        "module app.values; public enum Mode { Fast, Slow, } public let label = \"shared\";",
        r#"module app.main;
import app.values.Mode;
import app.values.label as Renamed;
import app.values as Values;
@custom(Mode.Fast, { Renamed }, Values)
agent Worker() -> i32 { return 1; }
flow main() -> Array<std.compiler.AnnotationInfo> { return (^Worker).annotations; }
"#,
    ]
    .into_iter()
    .enumerate()
    .map(|(id, text)| SourceInput {
        id: etas_core::SourceId(id as u32),
        path: None,
        text: text.into(),
        kind: etas_frontend::SourceKind::SourceProjectFile,
    })
    .collect();
    let (checked, definitions) = prepared_project(input, &[]);
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Array(annotations)) = result.value() else {
        panic!("missing annotations")
    };
    let annotations = annotations.borrow();
    use etas_code::descriptor::{AnnotationReferenceInfo as R, StaticValueInfo as S};
    assert!(
        matches!(static_argument(&annotations[0], 0).as_ref(), S::Reference(R::Declaration(key)) if key.kind() == etas_code::DefinitionKind::EnumVariant && key.module() == ["app", "values"] && key.declaration() == ["Mode", "Fast"])
    );
    let record = static_argument(&annotations[0], 1);
    let S::Record(fields) = record.as_ref() else {
        panic!("missing static record")
    };
    assert_eq!(fields[0].name, "Renamed");
    assert!(
        matches!(fields[0].value.as_ref(), S::Reference(R::Declaration(key)) if key.module() == ["app", "values"] && key.declaration() == ["label"])
    );
    assert!(
        matches!(static_argument(&annotations[0], 2).as_ref(), S::Reference(R::Module { path, .. }) if path == &["app", "values"])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_annotations_are_immutable_and_preserved_by_body_edits() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.compiler.parse_body;
@custom("unchanged", enabled = true)
agent Worker() -> i32 { return 1; }
agent Bare;
flow main() -> bool {
    let original = ^Worker;
    let body = match parse_body("return 2;") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    let annotation = match original.annotations.get(0) { Some(value) => value, None => { return false; }, };
    return original.annotations == draft.annotations && annotation.path.len() == 1
        && annotation.arguments.len() == 2 && (^Bare).annotations.len() == 0;
}
"#,
        &["app.Worker"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

fn static_argument(
    annotation: &InterpValue,
    index: usize,
) -> Arc<etas_code::descriptor::StaticValueInfo> {
    let InterpValue::Nominal { value, .. } = annotation else {
        panic!("missing checked AnnotationInfo")
    };
    let InterpValue::Record(record) = value.as_ref() else {
        panic!("missing annotation record")
    };
    let Some(InterpValue::Array(arguments)) = record.get("arguments") else {
        panic!("missing arguments")
    };
    let arguments = arguments.borrow();
    let InterpValue::Nominal { value, .. } = &arguments[index] else {
        panic!("missing checked AnnotationArgumentInfo")
    };
    let InterpValue::Record(record) = value.as_ref() else {
        panic!("missing argument record")
    };
    let Some(InterpValue::CodeMetadata(value)) = record.get("value") else {
        panic!("missing static value")
    };
    let crate::value::CodeMetadataPayload::StaticValue(value) = value.payload() else {
        panic!("incorrect static value role")
    };
    value.clone()
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_tool_descriptors_preserve_resolved_aliases_without_executing_tools() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = [
        "module app.left; public tool lookup(value: string) -> string { return value; }",
        "module app.right; public tool lookup(value: string) -> string { return value; }",
        r#"module app.main;
import app.left.lookup as First;
import app.right.lookup as Second;
@tools(items = [First, Second], choice = "auto")
agent Worker() -> i32 { return 1; }
flow main() -> Array<std.compiler.ToolRef> { return (^Worker).tools; }
"#,
    ]
    .into_iter()
    .enumerate()
    .map(|(id, text)| SourceInput {
        id: etas_core::SourceId(id as u32),
        path: None,
        text: text.into(),
        kind: etas_frontend::SourceKind::SourceProjectFile,
    })
    .collect();
    let (checked, definitions) = prepared_project(input, &[]);
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Array(tools)) = result.value() else {
        panic!("missing tool descriptors")
    };
    let identities = tools
        .borrow()
        .iter()
        .map(|tool| {
            let InterpValue::CodeMetadata(tool) = tool else {
                panic!("untyped tool metadata")
            };
            let crate::value::CodeMetadataPayload::Tool(identity) = tool.payload() else {
                panic!("incorrect metadata role")
            };
            assert_eq!(identity.kind(), etas_code::DefinitionKind::Tool);
            identity.clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(identities.len(), 2);
    assert_eq!(identities[0].declaration(), ["lookup"]);
    assert_eq!(identities[1].declaration(), ["lookup"]);
    assert_eq!(identities[0].module(), ["app", "left"]);
    assert_eq!(identities[1].module(), ["app", "right"]);
    assert_ne!(identities[0], identities[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_tool_surface_is_frozen_across_body_edits_and_absent_without_annotations() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.compiler.parse_body;
tool helper() -> i32 { return 1; }
agent Empty;
@tools([helper])
agent Worker() -> i32 { return 1; }
flow main() -> bool {
    let original = ^Worker;
    let body = match parse_body("return 2;") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    return original.tools.len() == 1 && original.tools == draft.tools && (^Empty).tools.len() == 0;
}
"#,
        &["app.Worker"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_return_type_json_exports_only_the_reflected_descriptor_closure() {
    let (checked, definitions) = prepared(
        r#"
module app;
type PrivatePayload = { private_catalog_field: string };
flow hidden(value: PrivatePayload) -> PrivatePayload { return value; }
agent Worker() -> i32 { return 1; }
flow main() -> std.compiler.TypeInfo { return (^Worker.run).signature.output; }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let value = result.value().unwrap();
    let InterpValue::CodeMetadata(metadata) = value else {
        panic!("missing TypeInfo")
    };
    let crate::value::CodeMetadataPayload::Type { graph, .. } = metadata.payload() else {
        panic!("incorrect descriptor role")
    };
    assert!(graph.data().types.iter().any(|ty| matches!(ty, etas_code::descriptor::TypeInfo::Record(fields) if fields.iter().any(|field| field.name == "private_catalog_field"))));
    let json = crate::api::codec::value_json(value);
    assert!(!json.to_string().contains("private_catalog_field"));
    let exported = &json["value"]["Type"];
    assert_eq!(exported["root"], 0);
    assert_eq!(
        exported["graph"]["types"],
        serde_json::json!([{"Primitive": "I32"}])
    );
    for kind in ["rows", "binders", "traces"] {
        assert!(exported["graph"][kind].as_array().unwrap().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_agent_methods_are_descriptors_and_drafts_invalidate_their_current_analysis() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.compiler.parse_body;
agent Worker() -> i32 ![] { return 1; }
impl agent Worker { public flow other() -> i32 ![] { return Worker(); } }
flow main() -> bool {
    let original = ^Worker;
    let body = match parse_body("return 2;") { Ok(body) => body, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    let old_method = match original.methods.get(0) { Some(method) => method, None => { return false; }, };
    let new_method = match draft.methods.get(0) { Some(method) => method, None => { return false; }, };
    return original.methods.len() == 1 && draft.methods.len() == 1
        && old_method.id == new_method.id && old_method.signature == new_method.signature
        && match old_method.body { Some(_) => true, None => false, }
        && match new_method.body { Some(_) => true, None => false, }
        && match old_method.effects.escaping { Some(_) => true, None => false, }
        && match new_method.effects.declared_bound { Some(_) => true, None => false, }
        && match new_method.effects.escaping { None => true, Some(_) => false, }
        && match new_method.effects.requested { None => true, Some(_) => false, }
        && match new_method.determinism { None => true, Some(_) => false, }
        && match new_method.trace { None => true, Some(_) => false, };
}
"#,
        &["app.Worker.other"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_agent_methods_apply_the_original_reflecting_modules_visibility() {
    let mut input = ProjectInput::single_source(SourceInput::anonymous(""));
    input.sources = [
        "module app.worker; public agent Worker() -> i32 { return 1; } impl agent Worker { public flow exposed() -> i32 { return 2; } flow hidden() -> i32 { return 3; } }",
        "module app.client; import app.worker.Worker; flow main() -> bool { let definition = ^Worker; let method = match definition.methods.get(0) { Some(method) => method, None => { return false; }, }; return definition.methods.len() == 1 && method.signature.parameters.len() == 0 && match method.body { Some(_) => true, None => false, }; }",
    ].into_iter().enumerate().map(|(id, text)| SourceInput { id: etas_core::SourceId(id as u32), path: None, text: text.into(), kind: etas_frontend::SourceKind::SourceProjectFile }).collect();
    let (checked, definitions) = prepared_project(
        input,
        &["app.worker.Worker.exposed", "app.worker.Worker.hidden"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_effect_fields_preserve_requested_actions_with_empty_escaping_effects() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.io.println;
agent Worker() -> unit { println("not executed")?; return; }
flow main() -> std.compiler.EffectSummary { return (^Worker).effects; }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Nominal { value, .. }) = result.value() else {
        panic!("missing EffectSummary");
    };
    let InterpValue::Record(summary) = value.as_ref() else {
        panic!("missing checked summary record");
    };
    assert_eq!(summary.get("declared_bound"), Some(InterpValue::OptionNone));
    for field in ["requested", "default_handled"] {
        let Some(InterpValue::OptionSome(value)) = summary.get(field) else {
            panic!("missing action summary");
        };
        let InterpValue::CodeMetadata(value) = value.as_ref() else {
            panic!("missing typed action metadata");
        };
        let crate::value::CodeMetadataPayload::Actions { graph, root } = value.payload() else {
            panic!("incorrect action metadata role");
        };
        let row = graph.effect_row(*root).unwrap();
        assert!(row.effects.iter().any(|effect| matches!(&effect.target, etas_code::descriptor::EffectTargetInfo::Action { name, .. } if name == "stdout_write")));
    }
    let Some(InterpValue::OptionSome(value)) = summary.get("escaping") else {
        panic!("missing known escaping row");
    };
    let InterpValue::CodeMetadata(value) = value.as_ref() else {
        panic!("missing escaping metadata");
    };
    let crate::value::CodeMetadataPayload::EffectRow { graph, root } = value.payload() else {
        panic!("incorrect escaping metadata role");
    };
    assert!(graph.effect_row(*root).unwrap().effects.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_definition_common_fields_share_the_checked_default_callable() {
    let (checked, definitions) = prepared(
        r#"
module app;
agent Worker(value: i32) -> i32 ![] { return value; }
flow main() -> bool {
    let agent = ^Worker;
    let callable = ^Worker.run;
    let signature = match agent.signature { Some(value) => value, None => { return false; }, };
    let parameter = match signature.parameters.get(0) { Some(value) => value, None => { return false; }, };
    let named = match parameter.name { Some(value) => value == "value", None => false, };
    return named && signature.generics.len() == 0 && signature.parameters.len() == 1
        && parameter.ty == signature.output
        && signature.output == callable.signature.output
        && match signature.declared_output { Some(value) => value == signature.output, None => false, }
        && match signature.effects { Some(_) => true, None => false, }
        && match agent.effects.declared_bound { Some(_) => true, None => false, }
        && match agent.effects.escaping { Some(_) => true, None => false, }
        && match agent.effects.requested { Some(_) => true, None => false, }
        && match agent.effects.default_handled { Some(_) => true, None => false, }
        && agent.effects == callable.effects
        && agent.determinism == callable.determinism
        && agent.trace == callable.trace
        && match agent.trace { Some(_) => true, None => false, }
        && agent.dependencies.environment != "";
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_callable_signature_preserves_generic_kinds_bounds_and_named_inputs() {
    let (checked, definitions) = prepared(
        r#"
module app;
flow identity<T ~ Index, effect E>(value: T) -> T ![E] { return value; }
flow inferred(value: i32) { return value; }
flow main() -> bool {
    let generic = (^identity).signature;
    let inferred = (^inferred).signature;
    let first = match generic.generics.get(0) { Some(value) => value, None => { return false; }, };
    let second = match generic.generics.get(1) { Some(value) => value, None => { return false; }, };
    let parameter = match generic.parameters.get(0) { Some(value) => value, None => { return false; }, };
    return generic.generics.len() == 2 && first.kind == "type" && first.name == "T"
        && first.position == 0 && first.bounds.len() == 1
        && second.kind == "effect" && second.name == "E" && second.position == 1
        && second.bounds.len() == 0 && first.owner == second.owner
        && match parameter.name { Some(value) => value == "value", None => false, }
        && parameter.ty == generic.output
        && match generic.declared_output { Some(value) => value == generic.output, None => false, }
        && match generic.effects { Some(_) => true, None => false, }
        && match inferred.declared_output { None => true, Some(_) => false, };
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_draft_fields_do_not_relabel_baseline_analysis_as_current() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.compiler.parse_body;
agent Worker(value: i32) -> i32 ![] { return value; }
flow main() -> bool {
    let original = ^Worker;
    let body = match parse_body("return value + 1;") { Ok(value) => value, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    return original.identity == draft.identity && original.dependencies == draft.dependencies
        && original.signature == draft.signature
        && match original.effects.escaping { Some(_) => true, None => false, }
        && match original.determinism { Some(_) => true, None => false, }
        && match original.trace { Some(_) => true, None => false, }
        && match draft.effects.declared_bound { Some(_) => true, None => false, }
        && match draft.effects.escaping { None => true, Some(_) => false, }
        && match draft.effects.requested { None => true, Some(_) => false, }
        && match draft.effects.default_handled { None => true, Some(_) => false, }
        && match draft.determinism { None => true, Some(_) => false, }
        && match draft.trace { None => true, Some(_) => false, }
        && original.revision.key != draft.revision.key;
}
"#,
        &["app.Worker"],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_bare_agent_draft_keeps_absent_signature_and_analysis() {
    let (checked, definitions) = prepared(
        r#"
module app;
import std.compiler.parse_body;
agent Bare;
flow main() -> bool {
    let original = ^Bare;
    let body = match parse_body("return 1;") { Ok(value) => value, Err(_) => { return false; }, };
    let draft = original.with_body(body);
    return original.identity == draft.identity
        && match original.signature { None => true, Some(_) => false, }
        && match original.body { None => true, Some(_) => false, }
        && match draft.signature { None => true, Some(_) => false, }
        && match draft.body { Some(_) => true, None => false, }
        && match draft.effects.declared_bound { None => true, Some(_) => false, }
        && match draft.effects.escaping { None => true, Some(_) => false, }
        && match draft.effects.requested { None => true, Some(_) => false, }
        && match draft.determinism { None => true, Some(_) => false, }
        && match draft.trace { None => true, Some(_) => false, };
}
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&InterpValue::Bool(true)));
}

#[tokio::test(flavor = "current_thread")]
async fn compiler_callable_id_and_agent_id_are_canonical_descriptive_values() {
    let (checked, definitions) = prepared(
        r#"
module app;
agent Worker() -> i32 { return 1; }
flow main() -> (std.compiler.AgentId, std.compiler.CallableId) { return ((^Worker).identity, (^Worker.run).id); }
"#,
        &[],
    );
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::Tuple(values)) = result.value() else {
        panic!("descriptor identities were not returned");
    };
    let InterpValue::CodeMetadata(agent) = &values[0] else {
        panic!("AgentId is not typed metadata");
    };
    let InterpValue::CodeMetadata(callable) = &values[1] else {
        panic!("CallableId is not typed metadata");
    };
    let crate::value::CodeMetadataPayload::Agent(identity) = agent.payload() else {
        panic!("incorrect identity kind");
    };
    let crate::value::CodeMetadataPayload::Callable(callable) = callable.payload() else {
        panic!("incorrect callable kind");
    };
    assert_eq!(
        callable.as_method().unwrap().implementation().subject(),
        identity
    );
    assert_eq!(
        agent.ty(),
        checked.types.known_std_types.code.agent_id.unwrap()
    );
}
