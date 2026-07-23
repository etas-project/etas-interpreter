pub(super) fn intrinsic_dispatch_name(dispatch: etas_std::IntrinsicDispatch) -> &'static str {
    match dispatch {
        etas_std::IntrinsicDispatch::PureKernel => "pure_kernel",
        etas_std::IntrinsicDispatch::Runtime => "runtime",
        etas_std::IntrinsicDispatch::Host => "host",
        etas_std::IntrinsicDispatch::LoweringOnly => "lowering_only",
    }
}

pub(super) fn intrinsic_dispatch_from_name(
    name: &str,
) -> Result<etas_std::IntrinsicDispatch, String> {
    match name {
        "pure_kernel" => Ok(etas_std::IntrinsicDispatch::PureKernel),
        "runtime" => Ok(etas_std::IntrinsicDispatch::Runtime),
        "host" => Ok(etas_std::IntrinsicDispatch::Host),
        "lowering_only" => Ok(etas_std::IntrinsicDispatch::LoweringOnly),
        _ => Err(format!("unknown machine intrinsic dispatch `{name}`")),
    }
}

pub(super) fn prompt_role_name(role: crate::value::PromptRole) -> &'static str {
    crate::value::codec::prompt_role_json(role)
}

pub(super) fn prompt_role_from_name(name: &str) -> Result<crate::value::PromptRole, String> {
    crate::value::codec::prompt_role_from_json(name)
}

pub(super) fn trust_wrapper_name(wrapper: etas_types::TrustWrapper) -> &'static str {
    crate::value::codec::trust_wrapper_json(wrapper)
}

pub(super) fn trust_wrapper_from_name(name: &str) -> Result<etas_types::TrustWrapper, String> {
    crate::value::codec::trust_wrapper_from_json(name)
}

pub(super) fn std_limit_kind_name(kind: etas_std::StdLimitKind) -> &'static str {
    match kind {
        etas_std::StdLimitKind::Iterations => "iterations",
        etas_std::StdLimitKind::Tokens => "tokens",
        etas_std::StdLimitKind::ContextTokens => "context_tokens",
        etas_std::StdLimitKind::Cost => "cost",
        etas_std::StdLimitKind::WallTime => "wall_time",
        etas_std::StdLimitKind::Attempts => "attempts",
    }
}

pub(super) fn std_limit_kind_from_name(name: &str) -> Result<etas_std::StdLimitKind, String> {
    match name {
        "iterations" => Ok(etas_std::StdLimitKind::Iterations),
        "tokens" => Ok(etas_std::StdLimitKind::Tokens),
        "context_tokens" => Ok(etas_std::StdLimitKind::ContextTokens),
        "cost" => Ok(etas_std::StdLimitKind::Cost),
        "wall_time" => Ok(etas_std::StdLimitKind::WallTime),
        "attempts" => Ok(etas_std::StdLimitKind::Attempts),
        _ => Err(format!("unknown machine std limit kind `{name}`")),
    }
}
