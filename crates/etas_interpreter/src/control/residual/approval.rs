/// Only the canonical standard action has trusted decision semantics. A source
/// declaration with the same spelling is not an approval authority.
pub(crate) fn is_standard_approval(
    registry: &etas_effects::EffectRegistry,
    effect: &etas_effects::Effect,
) -> bool {
    let action = match effect {
        etas_effects::Effect::Action(action) => action,
        etas_effects::Effect::AppliedAction(action) => &action.action,
        _ => return false,
    };
    registry
        .standard_action_by_name("Approval.request")
        .as_ref()
        == Some(action)
}
