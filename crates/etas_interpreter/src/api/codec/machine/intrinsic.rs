use serde_json::{Value, json};

use crate::intrinsic::dispatch::{
    BrowserCallable, CommandCallable, ConsoleCallable, FilesystemCallable, JsonCallable,
    SecretCallable, StdCallable, StreamCallable, TcpCallable, TlsCallable,
};

use super::value::{required_str, required_u32};

pub(super) fn std_callable_snapshot(callable: &StdCallable) -> Value {
    match callable {
        StdCallable::PureIntrinsic(id) => json!({ "kind": "pure_intrinsic", "id": id.0 }),
        StdCallable::OptionNoneConstructor => json!({ "kind": "option_none" }),
        StdCallable::MemoryVersionConstructor => json!({ "kind": "memory_version" }),
        StdCallable::StreamErrorHostConstructor => json!({ "kind": "stream_error_host" }),
        StdCallable::CurrentSession => json!({ "kind": "current_session" }),
        StdCallable::SessionPolicyConstructor(name) => {
            json!({ "kind": "session_policy", "name": name })
        }
        StdCallable::RuntimeLimitConstructor(kind) => {
            json!({ "kind": "runtime_limit", "limit_kind": std_limit_kind_name(*kind) })
        }
        StdCallable::TrustWrapper(wrapper) => json!({
            "kind": "trust_wrapper",
            "wrapper": crate::value::codec::trust_wrapper_json(*wrapper),
        }),
        StdCallable::Console(callable) => {
            json!({ "kind": "console", "operation": console_callable_name(*callable) })
        }
        StdCallable::Command(callable) => {
            json!({ "kind": "command", "operation": command_callable_name(*callable) })
        }
        StdCallable::Filesystem(callable) => json!({
            "kind": "filesystem",
            "operation": filesystem_callable_name(*callable),
        }),
        StdCallable::Tcp(callable) => {
            json!({ "kind": "tcp", "operation": tcp_callable_name(*callable) })
        }
        StdCallable::Stream(callable) => {
            json!({ "kind": "stream", "operation": stream_callable_name(*callable) })
        }
        StdCallable::Tls(callable) => {
            json!({ "kind": "tls", "operation": tls_callable_name(*callable) })
        }
        StdCallable::Secret(callable) => {
            json!({ "kind": "secret", "operation": secret_callable_name(*callable) })
        }
        StdCallable::Json(callable) => {
            json!({ "kind": "json", "operation": json_callable_name(*callable) })
        }
        StdCallable::Browser(callable) => {
            json!({ "kind": "browser", "operation": browser_callable_name(*callable) })
        }
    }
}

pub(super) fn std_callable_from_snapshot(value: &Value) -> Result<StdCallable, String> {
    match required_str(value, "kind")? {
        "pure_intrinsic" => Ok(StdCallable::PureIntrinsic(etas_std::StdIntrinsicId(
            required_u32(value, "id")?,
        ))),
        "option_none" => Ok(StdCallable::OptionNoneConstructor),
        "memory_version" => Ok(StdCallable::MemoryVersionConstructor),
        "stream_error_host" => Ok(StdCallable::StreamErrorHostConstructor),
        "current_session" => Ok(StdCallable::CurrentSession),
        "session_policy" => Ok(StdCallable::SessionPolicyConstructor(
            match required_str(value, "name")? {
                "LastTurns" => "LastTurns",
                "SummaryPlusRecent" => "SummaryPlusRecent",
                "Days" => "Days",
                "SummarizeWhen" => "SummarizeWhen",
                other => return Err(format!("unknown machine session policy `{other}`")),
            },
        )),
        "runtime_limit" => Ok(StdCallable::RuntimeLimitConstructor(
            std_limit_kind_from_name(required_str(value, "limit_kind")?)?,
        )),
        "trust_wrapper" => Ok(StdCallable::TrustWrapper(
            crate::value::codec::trust_wrapper_from_json(required_str(value, "wrapper")?)?,
        )),
        "console" => Ok(StdCallable::Console(console_callable_from_name(
            required_str(value, "operation")?,
        )?)),
        "command" => Ok(StdCallable::Command(command_callable_from_name(
            required_str(value, "operation")?,
        )?)),
        "filesystem" => Ok(StdCallable::Filesystem(filesystem_callable_from_name(
            required_str(value, "operation")?,
        )?)),
        "tcp" => Ok(StdCallable::Tcp(tcp_callable_from_name(required_str(
            value,
            "operation",
        )?)?)),
        "stream" => Ok(StdCallable::Stream(stream_callable_from_name(
            required_str(value, "operation")?,
        )?)),
        "tls" => Ok(StdCallable::Tls(tls_callable_from_name(required_str(
            value,
            "operation",
        )?)?)),
        "secret" => Ok(StdCallable::Secret(secret_callable_from_name(
            required_str(value, "operation")?,
        )?)),
        "json" => Ok(StdCallable::Json(json_callable_from_name(required_str(
            value,
            "operation",
        )?)?)),
        "browser" => Ok(StdCallable::Browser(browser_callable_from_name(
            required_str(value, "operation")?,
        )?)),
        other => Err(format!("unknown machine std callable `{other}`")),
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

pub(super) fn console_callable_name(value: ConsoleCallable) -> &'static str {
    match value {
        ConsoleCallable::ReadAll => "read_all",
        ConsoleCallable::ReadLine => "read_line",
        ConsoleCallable::Print => "print",
        ConsoleCallable::PrintLn => "println",
        ConsoleCallable::EPrintLn => "eprintln",
    }
}

pub(super) fn console_callable_from_name(name: &str) -> Result<ConsoleCallable, String> {
    match name {
        "read_all" => Ok(ConsoleCallable::ReadAll),
        "read_line" => Ok(ConsoleCallable::ReadLine),
        "print" => Ok(ConsoleCallable::Print),
        "println" => Ok(ConsoleCallable::PrintLn),
        "eprintln" => Ok(ConsoleCallable::EPrintLn),
        _ => Err(format!("unknown machine console callable `{name}`")),
    }
}

pub(super) fn command_callable_name(value: CommandCallable) -> &'static str {
    match value {
        CommandCallable::Run => "run",
    }
}

pub(super) fn command_callable_from_name(name: &str) -> Result<CommandCallable, String> {
    match name {
        "run" => Ok(CommandCallable::Run),
        _ => Err(format!("unknown machine command callable `{name}`")),
    }
}

pub(super) fn filesystem_callable_name(value: FilesystemCallable) -> &'static str {
    match value {
        FilesystemCallable::ReadBytes => "read_bytes",
        FilesystemCallable::WriteBytes => "write_bytes",
        FilesystemCallable::List => "list",
        FilesystemCallable::Stat => "stat",
        FilesystemCallable::AtomicReplace => "atomic_replace",
    }
}

pub(super) fn filesystem_callable_from_name(name: &str) -> Result<FilesystemCallable, String> {
    match name {
        "read_bytes" => Ok(FilesystemCallable::ReadBytes),
        "write_bytes" => Ok(FilesystemCallable::WriteBytes),
        "list" => Ok(FilesystemCallable::List),
        "stat" => Ok(FilesystemCallable::Stat),
        "atomic_replace" => Ok(FilesystemCallable::AtomicReplace),
        _ => Err(format!("unknown machine filesystem callable `{name}`")),
    }
}

pub(super) fn tcp_callable_name(value: TcpCallable) -> &'static str {
    match value {
        TcpCallable::Connect => "connect",
    }
}

pub(super) fn tcp_callable_from_name(name: &str) -> Result<TcpCallable, String> {
    match name {
        "connect" => Ok(TcpCallable::Connect),
        _ => Err(format!("unknown machine tcp callable `{name}`")),
    }
}

pub(super) fn stream_callable_name(value: StreamCallable) -> &'static str {
    match value {
        StreamCallable::Read => "read",
        StreamCallable::ReadUntilLimit => "read_until_limit",
        StreamCallable::WriteAll => "write_all",
        StreamCallable::Flush => "flush",
        StreamCallable::Close => "close",
    }
}

pub(super) fn stream_callable_from_name(name: &str) -> Result<StreamCallable, String> {
    match name {
        "read" => Ok(StreamCallable::Read),
        "read_until_limit" => Ok(StreamCallable::ReadUntilLimit),
        "write_all" => Ok(StreamCallable::WriteAll),
        "flush" => Ok(StreamCallable::Flush),
        "close" => Ok(StreamCallable::Close),
        _ => Err(format!("unknown machine stream callable `{name}`")),
    }
}

pub(super) fn tls_callable_name(value: TlsCallable) -> &'static str {
    match value {
        TlsCallable::Connect => "connect",
    }
}

pub(super) fn tls_callable_from_name(name: &str) -> Result<TlsCallable, String> {
    match name {
        "connect" => Ok(TlsCallable::Connect),
        _ => Err(format!("unknown machine tls callable `{name}`")),
    }
}

pub(super) fn secret_callable_name(value: SecretCallable) -> &'static str {
    match value {
        SecretCallable::Read => "read",
        SecretCallable::HmacSha256 => "hmac_sha256",
    }
}

pub(super) fn secret_callable_from_name(name: &str) -> Result<SecretCallable, String> {
    match name {
        "read" => Ok(SecretCallable::Read),
        "hmac_sha256" => Ok(SecretCallable::HmacSha256),
        _ => Err(format!("unknown machine secret callable `{name}`")),
    }
}

pub(super) fn json_callable_name(value: JsonCallable) -> &'static str {
    match value {
        JsonCallable::InvalidJson => "invalid_json",
        JsonCallable::Parse => "parse",
        JsonCallable::Stringify => "stringify",
    }
}

pub(super) fn json_callable_from_name(name: &str) -> Result<JsonCallable, String> {
    match name {
        "invalid_json" => Ok(JsonCallable::InvalidJson),
        "parse" => Ok(JsonCallable::Parse),
        "stringify" => Ok(JsonCallable::Stringify),
        _ => Err(format!("unknown machine json callable `{name}`")),
    }
}

pub(super) fn browser_callable_name(value: BrowserCallable) -> &'static str {
    match value {
        BrowserCallable::Attach => "attach",
        BrowserCallable::Create => "create",
        BrowserCallable::Send => "send",
        BrowserCallable::Recv => "recv",
        BrowserCallable::Screenshot => "screenshot",
        BrowserCallable::Close => "close",
    }
}

pub(super) fn browser_callable_from_name(name: &str) -> Result<BrowserCallable, String> {
    match name {
        "attach" => Ok(BrowserCallable::Attach),
        "create" => Ok(BrowserCallable::Create),
        "send" => Ok(BrowserCallable::Send),
        "recv" => Ok(BrowserCallable::Recv),
        "screenshot" => Ok(BrowserCallable::Screenshot),
        "close" => Ok(BrowserCallable::Close),
        _ => Err(format!("unknown machine browser callable `{name}`")),
    }
}
