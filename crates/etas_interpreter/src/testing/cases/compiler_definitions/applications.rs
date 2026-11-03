use super::*;

#[tokio::test(flavor = "current_thread")]
async fn bound_generic_flow_reflection_and_edit_preserve_the_applied_contract() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.{parse_body, diff};
flow identity<T>(value: T) -> T ![] { return abort("reflection must not invoke this flow"); }
flow boolean(value: bool) -> bool ![] { return value; }
flow main() -> (bool, bool, bool, bool, bool, bool, bool, bool) {
    let selected: (bool) -> bool ![] = identity;
    let original = ^selected;
    let body = match parse_body("return true;") { Ok(body) => body, Err(_) => { return abort("body parse failed"); }, };
    let draft = original.with_body(body);
    let changes = diff(original, draft);
    return (original.signature.generics.len() == 0,
        original.signature.output == (^boolean).signature.output,
        original.body != None, draft.body != None,
        original.effects.requested != None, draft.effects.requested == None,
        changes.body_changed == Some(true), !changes.signature_changed);
}
"#,
        &["app.main.identity"],
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
    assert_eq!(
        result.value(),
        Some(&InterpValue::Tuple(vec![InterpValue::Bool(true); 8].into()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bound_generic_flow_reflection_does_not_require_body_retention() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
flow identity<T>(value: T) -> T ![] { return value; }
flow main() -> bool {
    let selected: (bool) -> bool ![] = identity;
    let definition = ^selected;
    return definition.body == None && definition.signature.generics.len() == 0
        && definition.effects.escaping != None && definition.effects.requested != None;
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
async fn reflected_effect_row_binding_is_closed_without_evaluating_the_callback() {
    let (checked, definitions) = prepared(
        r#"
module app.main;
import std.compiler.CallableDef;
flow twice<effect E>(callback: () -> unit ![E]) -> unit ![E] {
    callback(); callback(); return;
}
flow main() -> CallableDef<(() -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write]> {
    let selected: (() -> unit ![Console.stdout_write]) -> unit ![Console.stdout_write] = twice;
    return ^selected;
}
"#,
        &[],
    );
    let backend = definitions.definitions().clone();
    let result = execute(
        &checked,
        RunOptions {
            code_definitions: Some(definitions),
            ..Default::default()
        },
    )
    .await;
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let Some(InterpValue::CallableDefinition(value)) = result.value() else {
        panic!("bound callable descriptor");
    };
    let view = backend.resolve(value.definition().unwrap()).unwrap();
    let frozen = view.environment().unwrap();
    let proof = frozen.callable_application().unwrap();
    assert!(
        matches!(proof.arguments(), [etas_types::CallableGenericArg::EffectRow(row)]
        if row.tail.is_none() && row.effects.len() == 1 && row.effects[0].name == "Console.stdout_write")
    );
    let analysis = view.current_analysis().unwrap();
    let graph = view.current_analysis_descriptors().unwrap();
    let escaping = graph
        .effect_row(analysis.effects.escaping.unwrap())
        .unwrap();
    assert_eq!(escaping.effects.len(), 1);
    assert!(escaping.tail.is_none());
    let Some(etas_code::descriptor::TraceInfo::Seq(parts)) = graph.trace(analysis.trace.unwrap())
    else {
        panic!("two callback call sites");
    };
    assert_eq!(parts.len(), 2);
    assert!(parts.iter().all(|part| matches!(
        graph.trace(*part),
        Some(etas_code::descriptor::TraceInfo::ParameterCall(_))
    )));
}
