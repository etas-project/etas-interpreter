use etas_std::{IntrinsicDescriptor, IntrinsicDispatch, StdIntrinsicId, StdLimitKind, intrinsic};
use etas_types::{TrustWrapper, TypeId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedPureIntrinsicCall {
    pub intrinsic: StdIntrinsicId,
    pub parameter_types: Vec<TypeId>,
    pub result_type: TypeId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StdIntrinsicIdentity {
    pub intrinsic: StdIntrinsicId,
    pub dispatch: IntrinsicDispatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedStdIntrinsicCall {
    pub identity: StdIntrinsicIdentity,
    pub parameter_types: Vec<TypeId>,
    pub result_type: TypeId,
}

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
    New,
    WithEnv,
    WithCwd,
    WithStdin,
    Run,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesystemCallable {
    Path,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryStoreCallable {
    Get,
    Put,
    PutVersioned,
    Contains,
    Keys,
    Insert,
    Delete,
    DeleteVersioned,
    Update,
    Clear,
    Select,
    Query,
    Scan,
    RelatedTo,
    Upsert,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StdCallable {
    Approval,
    Checkpoint,
    MemoryRegion,
    MemoryStore(MemoryStoreCallable),
    MemoryVersionConstructor,
    MoneyUsdConstructor,
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

pub(crate) fn std_callable_for_descriptor(descriptor: &IntrinsicDescriptor) -> Option<StdCallable> {
    let callable = match descriptor.id.0 {
        intrinsic::pure::JSON_INVALID_JSON => StdCallable::Json(JsonCallable::InvalidJson),
        intrinsic::pure::JSON_PARSE => StdCallable::Json(JsonCallable::Parse),
        intrinsic::pure::JSON_STRINGIFY => StdCallable::Json(JsonCallable::Stringify),
        intrinsic::pure::SESSION_LAST_TURNS => StdCallable::SessionPolicyConstructor("LastTurns"),
        intrinsic::pure::SESSION_SUMMARY_PLUS_RECENT => {
            StdCallable::SessionPolicyConstructor("SummaryPlusRecent")
        }
        intrinsic::pure::SESSION_DAYS => StdCallable::SessionPolicyConstructor("Days"),
        intrinsic::pure::SESSION_SUMMARIZE_WHEN => {
            StdCallable::SessionPolicyConstructor("SummarizeWhen")
        }
        intrinsic::pure::MEMORY_VERSION => StdCallable::MemoryVersionConstructor,
        intrinsic::pure::STREAM_ERROR_HOST => StdCallable::StreamErrorHostConstructor,
        intrinsic::pure::TRUST_TRUSTED => StdCallable::TrustWrapper(TrustWrapper::Trusted),
        intrinsic::pure::TRUST_UNTRUSTED => StdCallable::TrustWrapper(TrustWrapper::Untrusted),
        intrinsic::pure::TRUST_SECRET => StdCallable::TrustWrapper(TrustWrapper::Secret),
        intrinsic::pure::TRUST_PUBLIC => StdCallable::TrustWrapper(TrustWrapper::Public),
        intrinsic::pure::TRUST_SANITIZED => StdCallable::TrustWrapper(TrustWrapper::Sanitized),
        intrinsic::pure::LIMIT_ITERATIONS => {
            StdCallable::RuntimeLimitConstructor(StdLimitKind::Iterations)
        }
        intrinsic::pure::LIMIT_TOKENS => StdCallable::RuntimeLimitConstructor(StdLimitKind::Tokens),
        intrinsic::pure::LIMIT_CONTEXT_TOKENS => {
            StdCallable::RuntimeLimitConstructor(StdLimitKind::ContextTokens)
        }
        intrinsic::pure::LIMIT_COST => StdCallable::RuntimeLimitConstructor(StdLimitKind::Cost),
        intrinsic::pure::LIMIT_WALL_TIME => {
            StdCallable::RuntimeLimitConstructor(StdLimitKind::WallTime)
        }
        intrinsic::pure::LIMIT_ATTEMPTS => {
            StdCallable::RuntimeLimitConstructor(StdLimitKind::Attempts)
        }
        intrinsic::runtime::APPROVE => StdCallable::Approval,
        intrinsic::runtime::CHECKPOINT => StdCallable::Checkpoint,
        intrinsic::runtime::CURRENT_SESSION => StdCallable::CurrentSession,
        intrinsic::runtime::USD => StdCallable::MoneyUsdConstructor,
        intrinsic::runtime::IO_READ_ALL => StdCallable::Console(ConsoleCallable::ReadAll),
        intrinsic::runtime::IO_READ_LINE => StdCallable::Console(ConsoleCallable::ReadLine),
        intrinsic::runtime::IO_PRINT => StdCallable::Console(ConsoleCallable::Print),
        intrinsic::runtime::IO_PRINTLN => StdCallable::Console(ConsoleCallable::PrintLn),
        intrinsic::runtime::IO_EPRINTLN => StdCallable::Console(ConsoleCallable::EPrintLn),
        intrinsic::runtime::MEMORY_REGION => StdCallable::MemoryRegion,
        intrinsic::runtime::MEMORY_GET => StdCallable::MemoryStore(MemoryStoreCallable::Get),
        intrinsic::runtime::MEMORY_PUT => StdCallable::MemoryStore(MemoryStoreCallable::Put),
        intrinsic::runtime::MEMORY_PUT_VERSIONED => {
            StdCallable::MemoryStore(MemoryStoreCallable::PutVersioned)
        }
        intrinsic::runtime::MEMORY_CONTAINS => {
            StdCallable::MemoryStore(MemoryStoreCallable::Contains)
        }
        intrinsic::runtime::MEMORY_KEYS => StdCallable::MemoryStore(MemoryStoreCallable::Keys),
        intrinsic::runtime::MEMORY_INSERT => StdCallable::MemoryStore(MemoryStoreCallable::Insert),
        intrinsic::runtime::MEMORY_DELETE => StdCallable::MemoryStore(MemoryStoreCallable::Delete),
        intrinsic::runtime::MEMORY_DELETE_VERSIONED => {
            StdCallable::MemoryStore(MemoryStoreCallable::DeleteVersioned)
        }
        intrinsic::runtime::MEMORY_UPDATE => StdCallable::MemoryStore(MemoryStoreCallable::Update),
        intrinsic::runtime::MEMORY_CLEAR => StdCallable::MemoryStore(MemoryStoreCallable::Clear),
        intrinsic::runtime::MEMORY_SELECT => StdCallable::MemoryStore(MemoryStoreCallable::Select),
        intrinsic::runtime::MEMORY_QUERY => StdCallable::MemoryStore(MemoryStoreCallable::Query),
        intrinsic::runtime::MEMORY_SCAN => StdCallable::MemoryStore(MemoryStoreCallable::Scan),
        intrinsic::runtime::MEMORY_RELATED_TO => {
            StdCallable::MemoryStore(MemoryStoreCallable::RelatedTo)
        }
        intrinsic::runtime::MEMORY_UPSERT => StdCallable::MemoryStore(MemoryStoreCallable::Upsert),
        intrinsic::runtime::COMMAND_NEW => StdCallable::Command(CommandCallable::New),
        intrinsic::runtime::COMMAND_WITH_ENV => StdCallable::Command(CommandCallable::WithEnv),
        intrinsic::runtime::COMMAND_WITH_CWD => StdCallable::Command(CommandCallable::WithCwd),
        intrinsic::runtime::COMMAND_WITH_STDIN => StdCallable::Command(CommandCallable::WithStdin),
        intrinsic::runtime::COMMAND_RUN => StdCallable::Command(CommandCallable::Run),
        intrinsic::runtime::FS_PATH => StdCallable::Filesystem(FilesystemCallable::Path),
        intrinsic::runtime::FS_READ_BYTES => StdCallable::Filesystem(FilesystemCallable::ReadBytes),
        intrinsic::runtime::FS_WRITE_BYTES => {
            StdCallable::Filesystem(FilesystemCallable::WriteBytes)
        }
        intrinsic::runtime::FS_LIST => StdCallable::Filesystem(FilesystemCallable::List),
        intrinsic::runtime::FS_STAT => StdCallable::Filesystem(FilesystemCallable::Stat),
        intrinsic::runtime::FS_ATOMIC_REPLACE => {
            StdCallable::Filesystem(FilesystemCallable::AtomicReplace)
        }
        intrinsic::runtime::NET_TCP_CONNECT => StdCallable::Tcp(TcpCallable::Connect),
        intrinsic::runtime::STREAM_READ => StdCallable::Stream(StreamCallable::Read),
        intrinsic::runtime::STREAM_READ_UNTIL_LIMIT => {
            StdCallable::Stream(StreamCallable::ReadUntilLimit)
        }
        intrinsic::runtime::STREAM_WRITE_ALL => StdCallable::Stream(StreamCallable::WriteAll),
        intrinsic::runtime::STREAM_FLUSH => StdCallable::Stream(StreamCallable::Flush),
        intrinsic::runtime::STREAM_CLOSE => StdCallable::Stream(StreamCallable::Close),
        intrinsic::runtime::TLS_CONNECT => StdCallable::Tls(TlsCallable::Connect),
        intrinsic::runtime::SECRET_READ => StdCallable::Secret(SecretCallable::Read),
        intrinsic::runtime::SECRET_HMAC_SHA256 => StdCallable::Secret(SecretCallable::HmacSha256),
        intrinsic::runtime::BROWSER_ATTACH => StdCallable::Browser(BrowserCallable::Attach),
        intrinsic::runtime::BROWSER_CREATE => StdCallable::Browser(BrowserCallable::Create),
        intrinsic::runtime::BROWSER_SEND => StdCallable::Browser(BrowserCallable::Send),
        intrinsic::runtime::BROWSER_RECV => StdCallable::Browser(BrowserCallable::Recv),
        intrinsic::runtime::BROWSER_SCREENSHOT => StdCallable::Browser(BrowserCallable::Screenshot),
        intrinsic::runtime::BROWSER_CLOSE => StdCallable::Browser(BrowserCallable::Close),
        _ => return None,
    };
    Some(callable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn callable(path: &[&str]) -> Option<StdCallable> {
        let registry = etas_std::standard_registry();
        registry
            .lookup_qualified(path)
            .and_then(|symbol| symbol.intrinsic.as_ref())
            .and_then(std_callable_for_descriptor)
    }

    #[test]
    fn pure_intrinsics_are_owned_by_the_pure_kernel_registry() {
        assert_eq!(callable(&["std", "option", "None"]), None);
        assert_eq!(callable(&["std", "result", "unwrap"]), None);
    }

    #[test]
    fn resolves_host_callables_from_registry_descriptors() {
        for (path, expected) in [
            (
                &["std", "host", "command", "run"][..],
                StdCallable::Command(CommandCallable::Run),
            ),
            (
                &["std", "stream", "read"][..],
                StdCallable::Stream(StreamCallable::Read),
            ),
            (
                &["std", "stream", "read_until_limit"][..],
                StdCallable::Stream(StreamCallable::ReadUntilLimit),
            ),
            (
                &["std", "browser", "protocol", "screenshot"][..],
                StdCallable::Browser(BrowserCallable::Screenshot),
            ),
        ] {
            assert_eq!(callable(path), Some(expected));
        }
    }

    #[test]
    fn unresolved_path_has_no_descriptor_driven_callable() {
        assert_eq!(callable(&["std", "missing", "call"]), None);
    }
}
