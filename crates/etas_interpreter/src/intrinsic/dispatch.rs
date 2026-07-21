use etas_std::{StdIntrinsicId, StdLimitKind, intrinsic};
use etas_types::TrustWrapper;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleCallable {
    ReadAll,
    ReadLine,
    Print,
    PrintLn,
    EPrintLn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCallable {
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesystemCallable {
    ReadBytes,
    WriteBytes,
    List,
    Stat,
    AtomicReplace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpCallable {
    Connect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamCallable {
    Read,
    ReadUntilLimit,
    WriteAll,
    Flush,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TlsCallable {
    Connect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretCallable {
    Read,
    HmacSha256,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonCallable {
    InvalidJson,
    Parse,
    Stringify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserCallable {
    Attach,
    Create,
    Send,
    Recv,
    Screenshot,
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StdCallable {
    PureIntrinsic(StdIntrinsicId),
    OptionNoneConstructor,
    MemoryVersionConstructor,
    StreamErrorHostConstructor,
    CurrentSession,
    SessionPolicyConstructor(&'static str),
    RuntimeLimitConstructor(StdLimitKind),
    TrustWrapper(TrustWrapper),
    Console(ConsoleCallable),
    Command(CommandCallable),
    Filesystem(FilesystemCallable),
    Tcp(TcpCallable),
    Stream(StreamCallable),
    Tls(TlsCallable),
    Secret(SecretCallable),
    Json(JsonCallable),
    Browser(BrowserCallable),
}

pub(crate) fn std_callable_for_path(path: &[String]) -> Option<StdCallable> {
    match path {
        [std, collections, len] if std == "std" && collections == "collections" && len == "len" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::LIST_LEN,
            )))
        }
        [std, collections, is_empty]
            if std == "std" && collections == "collections" && is_empty == "is_empty" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::LIST_IS_EMPTY,
            )))
        }
        [std, collections, contains_key]
            if std == "std" && collections == "collections" && contains_key == "contains_key" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::MAP_CONTAINS_KEY,
            )))
        }
        [std, option, unwrap] if std == "std" && option == "option" && unwrap == "unwrap" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::OPTION_UNWRAP)),
        ),
        [std, option, some] if std == "std" && option == "option" && some == "Some" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::OPTION_SOME)),
        ),
        [std, option, none] if std == "std" && option == "option" && none == "None" => {
            Some(StdCallable::OptionNoneConstructor)
        }
        [std, option, is_some] if std == "std" && option == "option" && is_some == "is_some" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::OPTION_IS_SOME,
            )))
        }
        [std, option, is_none] if std == "std" && option == "option" && is_none == "is_none" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::OPTION_IS_NONE,
            )))
        }
        [std, result, ok] if std == "std" && result == "result" && ok == "Ok" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::RESULT_OK)),
        ),
        [std, result, err] if std == "std" && result == "result" && err == "Err" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::RESULT_ERR)),
        ),
        [std, result, is_ok] if std == "std" && result == "result" && is_ok == "is_ok" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::RESULT_IS_OK)),
        ),
        [std, result, is_err] if std == "std" && result == "result" && is_err == "is_err" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::RESULT_IS_ERR)),
        ),
        [std, bytes, len] if std == "std" && bytes == "bytes" && len == "len" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::BYTES_LEN)),
        ),
        [std, core, assert] if std == "std" && core == "core" && assert == "assert" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::ASSERT)),
        ),
        [std, core, abort] if std == "std" && core == "core" && abort == "abort" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::ABORT)),
        ),
        [std, text, trim] if std == "std" && text == "text" && trim == "trim" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::TEXT_TRIM)),
        ),
        [std, text, lowercase] if std == "std" && text == "text" && lowercase == "lowercase" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_LOWERCASE,
            )))
        }
        [std, text, uppercase] if std == "std" && text == "text" && uppercase == "uppercase" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_UPPERCASE,
            )))
        }
        [std, text, contains] if std == "std" && text == "text" && contains == "contains" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::TEXT_CONTAINS)),
        ),
        [std, text, starts_with]
            if std == "std" && text == "text" && starts_with == "starts_with" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_STARTS_WITH,
            )))
        }
        [std, text, ends_with] if std == "std" && text == "text" && ends_with == "ends_with" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_ENDS_WITH,
            )))
        }
        [std, text, len] if std == "std" && text == "text" && len == "len" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::TEXT_LEN)),
        ),
        [std, text, lines] if std == "std" && text == "text" && lines == "lines" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::TEXT_LINES)),
        ),
        [std, text, split] if std == "std" && text == "text" && split == "split" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::TEXT_SPLIT)),
        ),
        [std, text, join] if std == "std" && text == "text" && join == "join" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::TEXT_JOIN)),
        ),
        [std, text, to_string_i32]
            if std == "std" && text == "text" && to_string_i32 == "to_string_i32" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_TO_STRING_I32,
            )))
        }
        [std, text, to_string_usize]
            if std == "std" && text == "text" && to_string_usize == "to_string_usize" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_TO_STRING_USIZE,
            )))
        }
        [std, text, parse_i32] if std == "std" && text == "text" && parse_i32 == "parse_i32" => {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_PARSE_I32,
            )))
        }
        [std, http, codec, encode_request]
            if std == "std"
                && http == "http"
                && codec == "codec"
                && encode_request == "encode_request" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::HTTP_ENCODE_REQUEST,
            )))
        }
        [std, http, codec, decode_response_head]
            if std == "std"
                && http == "http"
                && codec == "codec"
                && decode_response_head == "decode_response_head" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::HTTP_DECODE_RESPONSE_HEAD,
            )))
        }
        [std, http, codec, decode_response]
            if std == "std"
                && http == "http"
                && codec == "codec"
                && decode_response == "decode_response" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::HTTP_DECODE_RESPONSE,
            )))
        }
        [std, codec, text, utf8_decode]
            if std == "std"
                && codec == "codec"
                && text == "text"
                && utf8_decode == "utf8_decode" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_UTF8_DECODE,
            )))
        }
        [std, codec, text, utf8_encode]
            if std == "std"
                && codec == "codec"
                && text == "text"
                && utf8_encode == "utf8_encode" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::TEXT_UTF8_ENCODE,
            )))
        }
        [std, crypto, sha256] if std == "std" && crypto == "crypto" && sha256 == "sha256" => Some(
            StdCallable::PureIntrinsic(StdIntrinsicId(intrinsic::pure::CRYPTO_SHA256)),
        ),
        [std, crypto, constant_time_eq]
            if std == "std" && crypto == "crypto" && constant_time_eq == "constant_time_eq" =>
        {
            Some(StdCallable::PureIntrinsic(StdIntrinsicId(
                intrinsic::pure::CRYPTO_CONSTANT_TIME_EQ,
            )))
        }
        [std, json, parse] if std == "std" && json == "json" && parse == "parse" => {
            Some(StdCallable::Json(JsonCallable::Parse))
        }
        [std, json, stringify] if std == "std" && json == "json" && stringify == "stringify" => {
            Some(StdCallable::Json(JsonCallable::Stringify))
        }
        [std, json, invalid_json]
            if std == "std" && json == "json" && invalid_json == "InvalidJson" =>
        {
            Some(StdCallable::Json(JsonCallable::InvalidJson))
        }
        [std, agent, session, name]
            if std == "std"
                && agent == "agent"
                && session == "session"
                && name == "current_session" =>
        {
            Some(StdCallable::CurrentSession)
        }
        [std, agent, session, name]
            if std == "std"
                && agent == "agent"
                && session == "session"
                && matches!(
                    name.as_str(),
                    "LastTurns" | "SummaryPlusRecent" | "Days" | "SummarizeWhen"
                ) =>
        {
            Some(StdCallable::SessionPolicyConstructor(match name.as_str() {
                "LastTurns" => "LastTurns",
                "SummaryPlusRecent" => "SummaryPlusRecent",
                "Days" => "Days",
                "SummarizeWhen" => "SummarizeWhen",
                _ => unreachable!(),
            }))
        }
        [std, runtime, limits, name]
            if std == "std"
                && runtime == "runtime"
                && limits == "limits"
                && matches!(
                    name.as_str(),
                    "Iterations" | "Tokens" | "ContextTokens" | "Cost" | "WallTime" | "Attempts"
                ) =>
        {
            Some(StdCallable::RuntimeLimitConstructor(match name.as_str() {
                "Iterations" => StdLimitKind::Iterations,
                "Tokens" => StdLimitKind::Tokens,
                "ContextTokens" => StdLimitKind::ContextTokens,
                "Cost" => StdLimitKind::Cost,
                "WallTime" => StdLimitKind::WallTime,
                "Attempts" => StdLimitKind::Attempts,
                _ => unreachable!(),
            }))
        }
        [std, memory, version] if std == "std" && memory == "memory" && version == "version" => {
            Some(StdCallable::MemoryVersionConstructor)
        }
        [std, stream, host] if std == "std" && stream == "stream" && host == "Host" => {
            Some(StdCallable::StreamErrorHostConstructor)
        }
        [std, security, trust, wrapper]
            if std == "std"
                && security == "security"
                && trust == "trust"
                && matches!(
                    wrapper.as_str(),
                    "Trusted" | "Untrusted" | "Secret" | "Public" | "Sanitized"
                ) =>
        {
            Some(StdCallable::TrustWrapper(match wrapper.as_str() {
                "Trusted" => TrustWrapper::Trusted,
                "Untrusted" => TrustWrapper::Untrusted,
                "Secret" => TrustWrapper::Secret,
                "Public" => TrustWrapper::Public,
                "Sanitized" => TrustWrapper::Sanitized,
                _ => unreachable!(),
            }))
        }
        [std, io, read_all] if std == "std" && io == "io" && read_all == "read_all" => {
            Some(StdCallable::Console(ConsoleCallable::ReadAll))
        }
        [std, io, read_line] if std == "std" && io == "io" && read_line == "read_line" => {
            Some(StdCallable::Console(ConsoleCallable::ReadLine))
        }
        [std, io, print] if std == "std" && io == "io" && print == "print" => {
            Some(StdCallable::Console(ConsoleCallable::Print))
        }
        [std, io, println] if std == "std" && io == "io" && println == "println" => {
            Some(StdCallable::Console(ConsoleCallable::PrintLn))
        }
        [std, io, eprintln] if std == "std" && io == "io" && eprintln == "eprintln" => {
            Some(StdCallable::Console(ConsoleCallable::EPrintLn))
        }
        [std, host, command, run]
            if std == "std" && host == "host" && command == "command" && run == "run" =>
        {
            Some(StdCallable::Command(CommandCallable::Run))
        }
        [std, fs, read_bytes] if std == "std" && fs == "fs" && read_bytes == "read_bytes" => {
            Some(StdCallable::Filesystem(FilesystemCallable::ReadBytes))
        }
        [std, fs, write_bytes] if std == "std" && fs == "fs" && write_bytes == "write_bytes" => {
            Some(StdCallable::Filesystem(FilesystemCallable::WriteBytes))
        }
        [std, fs, list] if std == "std" && fs == "fs" && list == "list" => {
            Some(StdCallable::Filesystem(FilesystemCallable::List))
        }
        [std, fs, stat] if std == "std" && fs == "fs" && stat == "stat" => {
            Some(StdCallable::Filesystem(FilesystemCallable::Stat))
        }
        [std, fs, atomic_replace]
            if std == "std" && fs == "fs" && atomic_replace == "atomic_replace" =>
        {
            Some(StdCallable::Filesystem(FilesystemCallable::AtomicReplace))
        }
        [std, net, tcp, connect]
            if std == "std" && net == "net" && tcp == "tcp" && connect == "connect" =>
        {
            Some(StdCallable::Tcp(TcpCallable::Connect))
        }
        [std, stream, read] if std == "std" && stream == "stream" && read == "read" => {
            Some(StdCallable::Stream(StreamCallable::Read))
        }
        [std, stream, read_until_limit]
            if std == "std" && stream == "stream" && read_until_limit == "read_until_limit" =>
        {
            Some(StdCallable::Stream(StreamCallable::ReadUntilLimit))
        }
        [std, stream, write_all]
            if std == "std" && stream == "stream" && write_all == "write_all" =>
        {
            Some(StdCallable::Stream(StreamCallable::WriteAll))
        }
        [std, stream, flush] if std == "std" && stream == "stream" && flush == "flush" => {
            Some(StdCallable::Stream(StreamCallable::Flush))
        }
        [std, stream, close] if std == "std" && stream == "stream" && close == "close" => {
            Some(StdCallable::Stream(StreamCallable::Close))
        }
        [std, tls, connect] if std == "std" && tls == "tls" && connect == "connect" => {
            Some(StdCallable::Tls(TlsCallable::Connect))
        }
        [std, secret, read] if std == "std" && secret == "secret" && read == "read" => {
            Some(StdCallable::Secret(SecretCallable::Read))
        }
        [std, crypto, hmac_sha256]
            if std == "std" && crypto == "crypto" && hmac_sha256 == "hmac_sha256" =>
        {
            Some(StdCallable::Secret(SecretCallable::HmacSha256))
        }
        [std, browser, protocol, attach]
            if std == "std"
                && browser == "browser"
                && protocol == "protocol"
                && attach == "attach" =>
        {
            Some(StdCallable::Browser(BrowserCallable::Attach))
        }
        [std, browser, protocol, create]
            if std == "std"
                && browser == "browser"
                && protocol == "protocol"
                && create == "create" =>
        {
            Some(StdCallable::Browser(BrowserCallable::Create))
        }
        [std, browser, protocol, send]
            if std == "std" && browser == "browser" && protocol == "protocol" && send == "send" =>
        {
            Some(StdCallable::Browser(BrowserCallable::Send))
        }
        [std, browser, protocol, recv]
            if std == "std" && browser == "browser" && protocol == "protocol" && recv == "recv" =>
        {
            Some(StdCallable::Browser(BrowserCallable::Recv))
        }
        [std, browser, protocol, screenshot]
            if std == "std"
                && browser == "browser"
                && protocol == "protocol"
                && screenshot == "screenshot" =>
        {
            Some(StdCallable::Browser(BrowserCallable::Screenshot))
        }
        [std, browser, protocol, close]
            if std == "std"
                && browser == "browser"
                && protocol == "protocol"
                && close == "close" =>
        {
            Some(StdCallable::Browser(BrowserCallable::Close))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_std_codec_and_crypto_helpers_to_pure_intrinsics() {
        let cases: &[(&[&str], u32)] = &[
            (
                &["std", "http", "codec", "encode_request"],
                intrinsic::pure::HTTP_ENCODE_REQUEST,
            ),
            (
                &["std", "http", "codec", "decode_response_head"],
                intrinsic::pure::HTTP_DECODE_RESPONSE_HEAD,
            ),
            (
                &["std", "http", "codec", "decode_response"],
                intrinsic::pure::HTTP_DECODE_RESPONSE,
            ),
            (
                &["std", "codec", "text", "utf8_decode"],
                intrinsic::pure::TEXT_UTF8_DECODE,
            ),
            (
                &["std", "codec", "text", "utf8_encode"],
                intrinsic::pure::TEXT_UTF8_ENCODE,
            ),
            (&["std", "bytes", "len"], intrinsic::pure::BYTES_LEN),
            (&["std", "option", "Some"], intrinsic::pure::OPTION_SOME),
            (
                &["std", "option", "is_some"],
                intrinsic::pure::OPTION_IS_SOME,
            ),
            (
                &["std", "option", "is_none"],
                intrinsic::pure::OPTION_IS_NONE,
            ),
            (&["std", "option", "unwrap"], intrinsic::pure::OPTION_UNWRAP),
            (&["std", "result", "Ok"], intrinsic::pure::RESULT_OK),
            (&["std", "result", "Err"], intrinsic::pure::RESULT_ERR),
            (&["std", "result", "is_ok"], intrinsic::pure::RESULT_IS_OK),
            (&["std", "result", "is_err"], intrinsic::pure::RESULT_IS_ERR),
            (&["std", "crypto", "sha256"], intrinsic::pure::CRYPTO_SHA256),
            (
                &["std", "crypto", "constant_time_eq"],
                intrinsic::pure::CRYPTO_CONSTANT_TIME_EQ,
            ),
        ];
        for (path, expected) in cases {
            let path = path
                .iter()
                .map(|segment| (*segment).to_owned())
                .collect::<Vec<_>>();
            assert_eq!(
                std_callable_for_path(&path),
                Some(StdCallable::PureIntrinsic(StdIntrinsicId(*expected)))
            );
        }
    }

    #[test]
    fn maps_std_host_domain_callables() {
        let cases: &[(&[&str], StdCallable)] = &[
            (
                &["std", "fs", "read_bytes"],
                StdCallable::Filesystem(FilesystemCallable::ReadBytes),
            ),
            (
                &["std", "fs", "write_bytes"],
                StdCallable::Filesystem(FilesystemCallable::WriteBytes),
            ),
            (
                &["std", "fs", "list"],
                StdCallable::Filesystem(FilesystemCallable::List),
            ),
            (
                &["std", "fs", "stat"],
                StdCallable::Filesystem(FilesystemCallable::Stat),
            ),
            (
                &["std", "fs", "atomic_replace"],
                StdCallable::Filesystem(FilesystemCallable::AtomicReplace),
            ),
            (
                &["std", "net", "tcp", "connect"],
                StdCallable::Tcp(TcpCallable::Connect),
            ),
            (
                &["std", "stream", "read"],
                StdCallable::Stream(StreamCallable::Read),
            ),
            (
                &["std", "stream", "read_until_limit"],
                StdCallable::Stream(StreamCallable::ReadUntilLimit),
            ),
            (
                &["std", "stream", "write_all"],
                StdCallable::Stream(StreamCallable::WriteAll),
            ),
            (
                &["std", "stream", "flush"],
                StdCallable::Stream(StreamCallable::Flush),
            ),
            (
                &["std", "stream", "close"],
                StdCallable::Stream(StreamCallable::Close),
            ),
            (
                &["std", "tls", "connect"],
                StdCallable::Tls(TlsCallable::Connect),
            ),
            (
                &["std", "secret", "read"],
                StdCallable::Secret(SecretCallable::Read),
            ),
            (
                &["std", "crypto", "hmac_sha256"],
                StdCallable::Secret(SecretCallable::HmacSha256),
            ),
            (
                &["std", "json", "parse"],
                StdCallable::Json(JsonCallable::Parse),
            ),
            (
                &["std", "json", "stringify"],
                StdCallable::Json(JsonCallable::Stringify),
            ),
            (
                &["std", "json", "InvalidJson"],
                StdCallable::Json(JsonCallable::InvalidJson),
            ),
            (
                &["std", "browser", "protocol", "attach"],
                StdCallable::Browser(BrowserCallable::Attach),
            ),
            (
                &["std", "browser", "protocol", "create"],
                StdCallable::Browser(BrowserCallable::Create),
            ),
            (
                &["std", "browser", "protocol", "send"],
                StdCallable::Browser(BrowserCallable::Send),
            ),
            (
                &["std", "browser", "protocol", "recv"],
                StdCallable::Browser(BrowserCallable::Recv),
            ),
            (
                &["std", "browser", "protocol", "screenshot"],
                StdCallable::Browser(BrowserCallable::Screenshot),
            ),
            (
                &["std", "browser", "protocol", "close"],
                StdCallable::Browser(BrowserCallable::Close),
            ),
        ];
        for (path, expected) in cases {
            let path = path
                .iter()
                .map(|segment| segment.to_string())
                .collect::<Vec<_>>();
            assert_eq!(std_callable_for_path(&path), Some(expected.clone()));
        }
    }

    #[test]
    fn maps_std_option_none_constructor() {
        let path = ["std", "option", "None"]
            .iter()
            .map(|segment| (*segment).to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            std_callable_for_path(&path),
            Some(StdCallable::OptionNoneConstructor)
        );
    }
}
