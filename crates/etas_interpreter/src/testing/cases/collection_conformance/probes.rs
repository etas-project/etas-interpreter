use super::*;
use etas_std::{FlowSourceMethodKind, StdDecl, StdType};

fn fixture(ty: &StdType) -> String {
    match ty {
        StdType::Var(name) => match name.as_str() {
            "I" => "1",
            "K" => "4",
            "P" => "8",
            _ => "9",
        }
        .into(),
        StdType::Array(_) | StdType::Support(_) => "[4, 5]".into(),
        StdType::List(_) => "[4; 5]".into(),
        StdType::Slice(_) => "[4, 5][0, 2)".into(),
        StdType::Map { .. } => "{4 => 5}".into(),
        StdType::Function { .. } => "compare".into(),
        StdType::NamedApplied { name, .. } => match name.as_str() {
            "Deque" => "Deque.new<i32>().push_back(4)",
            "Queue" => "Queue.new<i32>().push(4)",
            "Stack" => "Stack.new<i32>().push(4)",
            "PriorityQueue" => "PriorityQueue.new<i32, i32>().push(4, 5)",
            "OrderedMap" => "OrderedMap.new<i32, i32>().insert(4, 5)",
            "OrderedSet" => "OrderedSet.new<i32>().insert(4)",
            _ => panic!("missing conformance fixture for {ty:?}"),
        }
        .into(),
        _ => panic!("missing conformance fixture for {ty:?}"),
    }
}

fn type_member(ty: &StdType) -> &str {
    match ty {
        StdType::Range(_) => "Range",
        StdType::NamedApplied { name, .. } => name,
        _ => panic!("unrecognized type member {ty:?}"),
    }
}

fn generic_args(flow: &etas_std::FlowDecl) -> String {
    if flow.type_params.is_empty() {
        String::new()
    } else {
        format!("<{}>", vec!["i32"; flow.type_params.len()].join(", "))
    }
}

fn source(body: &str) -> String {
    format!(
        "module app.main; import std.collections; flow compare(a: i32, b: i32) -> i32 {{ return a - b; }} flow main() -> bool {{ {body} }}"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn all_exported_collection_functions_agree_with_their_checked_method_operations() {
    let registry = etas_std::standard_registry();
    for case in cases() {
        eprintln!("direct collection function: {}", case.symbol);
        let symbol = registry
            .lookup_qualified(&["std", "collections", case.symbol])
            .unwrap();
        let StdDecl::Flow(flow) = &symbol.decl else {
            panic!("flow")
        };
        let args = flow
            .params
            .iter()
            .enumerate()
            .map(|(index, ty)| format!("let arg{index} = {};", fixture(ty)))
            .collect::<String>();
        let names = (0..flow.params.len())
            .map(|index| format!("arg{index}"))
            .collect::<Vec<_>>();
        let generics = generic_args(flow);
        let method = match &flow.source_method {
            Some(method) if method.kind == FlowSourceMethodKind::TypeMember => format!(
                "{}.{}{generics}({})",
                type_member(&method.receiver),
                method.name,
                names.join(", ")
            ),
            method => format!(
                "arg0.{}({})",
                method
                    .as_ref()
                    .map_or(flow.name.as_str(), |method| method.name.as_str()),
                names[1..].join(", ")
            ),
        };
        let capture = if flow.public_effects.is_empty() {
            ""
        } else {
            "?"
        };
        let checked = checked_project(&source(&format!(
            "{args} let actual = std.collections.{}{generics}({}){capture}; let expected = {method}{capture}; return actual == expected;",
            case.symbol,
            names.join(", ")
        )));
        let result = Interpreter
            .run_checked(
                &checked,
                EntryPoint {
                    item: checked.entry.unwrap(),
                },
                vec![],
                &FakeHost::new(availability(&[])),
                RunOptions::default(),
            )
            .await
            .unwrap();
        assert!(
            result.diagnostics.is_empty(),
            "{}: {:?}",
            case.symbol,
            result.diagnostics
        );
        assert_eq!(
            result.value(),
            Some(&InterpValue::Bool(true)),
            "{}",
            case.symbol
        );
    }
}

fn check_invalid(text: &str, invocation: &str) {
    use etas_frontend::{
        Frontend, ModulePath, ProjectCompileOptions, ProjectEntry, ProjectInput, SourceInput,
        SourceKind,
    };
    let output = Frontend.check_project(ProjectInput {
        project_root: "/workspace/demo".into(),
        source_root: Some("/workspace/demo/src".into()),
        options: ProjectCompileOptions::default(),
        environment: ProjectEnvironmentInput::default(),
        sources: vec![SourceInput {
            id: SourceId(0),
            path: Some("/workspace/demo/src/app/main.es".into()),
            text: text.into(),
            kind: SourceKind::SourceProjectFile,
        }],
        entry: ProjectEntry {
            module: Some(ModulePath {
                segments: vec!["app".into(), "main".into()],
            }),
            flow: "main".into(),
        },
    });
    let begin = text.find(invocation).unwrap();
    assert!(
        output.checked.is_none(),
        "invalid call accepted: {invocation}"
    );
    assert!(
        !output.diagnostics.is_empty(),
        "missing diagnostic: {invocation}"
    );
    for diagnostic in &output.diagnostics {
        assert!(
            matches!(diagnostic.code, DiagnosticCode::Type(_)),
            "{invocation}: {diagnostic:?}"
        );
        let span = diagnostic.primary.span;
        assert_eq!(span.source, SourceId(0));
        assert!(
            span.range.start.0 as usize >= begin
                && span.range.end.0 as usize <= begin + invocation.len() + 1,
            "{invocation}: {diagnostic:?}"
        );
    }
}

#[test]
fn registry_collection_calls_reject_wrong_parameter_types_and_argument_counts() {
    let registry = etas_std::standard_registry();
    for case in cases() {
        let symbol = registry
            .lookup_qualified(&["std", "collections", case.symbol])
            .unwrap();
        let StdDecl::Flow(flow) = &symbol.decl else {
            panic!("flow")
        };
        let args = flow.params.iter().map(fixture).collect::<Vec<_>>();
        for index in 0..=args.len() {
            let mut invalid = args.clone();
            if index == args.len() {
                invalid.push("false".into());
            } else {
                invalid[index] = "false".into();
            }
            let invocation = format!(
                "std.collections.{}{}({})",
                case.symbol,
                generic_args(flow),
                invalid.join(", ")
            );
            check_invalid(&source(&format!("{invocation}; return true;")), &invocation);
        }
    }
}
