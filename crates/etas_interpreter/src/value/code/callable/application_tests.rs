use super::*;

fn checked(source: &str) -> CheckedProgram {
    let mut input =
        etas_frontend::ProjectInput::single_source(etas_frontend::SourceInput::anonymous(source));
    input.options.entry_policy = etas_frontend::EntryPolicy::Optional;
    let output = etas_frontend::Frontend.check_project(input);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    output.checked.unwrap()
}

fn nominal(checked: &CheckedProgram, name: &str) -> TypeId {
    let symbol = checked
        .symbols
        .iter()
        .find(|symbol| symbol.name == name)
        .unwrap()
        .id;
    match &checked.types.symbol_types[&symbol] {
        etas_types::SymbolTypeFact::NominalType { constructor, .. }
        | etas_types::SymbolTypeFact::Type { constructor } => TypeId(constructor.0),
        _ => panic!("nominal declaration"),
    }
}

#[test]
fn a_matching_arrow_cannot_authorize_an_unchecked_phantom_application() {
    let checked = checked(
        "module app;
         type Allowed;
         type Unchecked;
         flow phantom<R>() -> i32 ![] { return 0; }
         flow prepare() -> unit ![] { phantom<Allowed>(); return; }",
    );
    let item = checked
        .hir
        .items
        .iter()
        .find_map(|(id, node)| match node {
            etas_hir::HirItem::Flow(flow)
                if checked.hir.symbols.get(flow.symbol).unwrap().name == "phantom" =>
            {
                Some(id)
            }
            _ => None,
        })
        .unwrap();
    let unchecked = nominal(&checked, "Unchecked");
    let Some(etas_types::ItemSignature::Flow(signature)) = checked.types.item_signatures.get(&item)
    else {
        panic!("flow signature");
    };
    let expected = FlowType {
        input: signature.params.clone(),
        output: signature.output,
        effects: signature.effects.clone(),
    };
    let target = CallTarget::Specialized {
        target: CallTarget::FlowItem(item).into(),
        type_bindings: vec![("R".into(), unchecked)],
        effect_row_bindings: vec![],
    };
    assert!(
        validate_signature(
            &checked,
            CheckedCallableRoot::Item(item),
            &target,
            &expected
        )
        .is_err()
    );
}

#[test]
fn a_closure_retains_its_checked_lexical_generic_application() {
    let checked = checked(
        "module app;
        type Allowed; type Unchecked;
        flow factory<R>() -> () -> i32 ![] ![] { return () => 0; }
        flow prepare() -> unit ![] { let f = factory<Allowed>(); return; }",
    );
    let expr = checked
        .exprs
        .iter()
        .find_map(|(id, expr)| matches!(expr, etas_hir::HirExpr::Lambda { .. }).then_some(id))
        .unwrap();
    let Some(Type::Function(signature)) = checked.type_store.get(checked.types.expr_types[&expr])
    else {
        panic!("closure signature");
    };
    let target = |ty| CallTarget::Lambda {
        expr,
        captured: crate::control::Frame::with_type_bindings(
            Arc::new(crate::plan::SlotLayoutTable::default()),
            [("R".into(), ty)].into_iter().collect(),
        ),
    };
    let allowed = nominal(&checked, "Allowed");
    let application = validate_signature(
        &checked,
        CheckedCallableRoot::Lambda(expr),
        &target(allowed),
        signature,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        application.arguments(),
        &[etas_types::CallableGenericArg::Type(allowed)]
    );
    assert_eq!(
        native_application_owner(&checked, CheckedCallableRoot::Lambda(expr)).unwrap(),
        Some(application.owner())
    );
    assert!(
        validate_signature(
            &checked,
            CheckedCallableRoot::Lambda(expr),
            &target(nominal(&checked, "Unchecked")),
            signature
        )
        .is_err()
    );
    let conflicting = CallTarget::Specialized {
        target: target(allowed).into(),
        type_bindings: vec![("R".into(), nominal(&checked, "Unchecked"))],
        effect_row_bindings: vec![],
    };
    assert!(
        validate_signature(
            &checked,
            CheckedCallableRoot::Lambda(expr),
            &conflicting,
            signature
        )
        .unwrap_err()
        .contains("conflicting type arguments")
    );
}
