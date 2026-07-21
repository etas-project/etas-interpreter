use crate::intrinsic::dispatch::StdCallable;
use crate::orchestration::StdCallableSnapshot;

pub(super) fn capture_std_callable(callable: &StdCallable) -> StdCallableSnapshot {
    match callable {
        StdCallable::PureIntrinsic(id) => StdCallableSnapshot::PureIntrinsic(*id),
        StdCallable::OptionNoneConstructor => StdCallableSnapshot::OptionNoneConstructor,
        StdCallable::MemoryVersionConstructor => StdCallableSnapshot::MemoryVersionConstructor,
        StdCallable::StreamErrorHostConstructor => StdCallableSnapshot::StreamErrorHostConstructor,
        StdCallable::CurrentSession => StdCallableSnapshot::CurrentSession,
        StdCallable::SessionPolicyConstructor(name) => {
            StdCallableSnapshot::SessionPolicyConstructor((*name).to_owned())
        }
        StdCallable::RuntimeLimitConstructor(kind) => {
            StdCallableSnapshot::RuntimeLimitConstructor(*kind)
        }
        StdCallable::TrustWrapper(wrapper) => StdCallableSnapshot::TrustWrapper(*wrapper),
        StdCallable::Console(callable) => StdCallableSnapshot::Console(*callable),
        StdCallable::Command(callable) => StdCallableSnapshot::Command(*callable),
        StdCallable::Filesystem(callable) => StdCallableSnapshot::Filesystem(*callable),
        StdCallable::Tcp(callable) => StdCallableSnapshot::Tcp(*callable),
        StdCallable::Stream(callable) => StdCallableSnapshot::Stream(*callable),
        StdCallable::Tls(callable) => StdCallableSnapshot::Tls(*callable),
        StdCallable::Secret(callable) => StdCallableSnapshot::Secret(*callable),
        StdCallable::Json(callable) => StdCallableSnapshot::Json(*callable),
        StdCallable::Browser(callable) => StdCallableSnapshot::Browser(*callable),
    }
}

pub(super) fn restore_std_callable(snapshot: StdCallableSnapshot) -> Result<StdCallable, String> {
    Ok(match snapshot {
        StdCallableSnapshot::PureIntrinsic(id) => StdCallable::PureIntrinsic(id),
        StdCallableSnapshot::OptionNoneConstructor => StdCallable::OptionNoneConstructor,
        StdCallableSnapshot::MemoryVersionConstructor => StdCallable::MemoryVersionConstructor,
        StdCallableSnapshot::StreamErrorHostConstructor => StdCallable::StreamErrorHostConstructor,
        StdCallableSnapshot::CurrentSession => StdCallable::CurrentSession,
        StdCallableSnapshot::SessionPolicyConstructor(name) => {
            StdCallable::SessionPolicyConstructor(match name.as_str() {
                "LastTurns" => "LastTurns",
                "SummaryPlusRecent" => "SummaryPlusRecent",
                "Days" => "Days",
                "SummarizeWhen" => "SummarizeWhen",
                other => {
                    return Err(format!(
                        "checkpoint contains unknown session policy constructor `{other}`"
                    ));
                }
            })
        }
        StdCallableSnapshot::RuntimeLimitConstructor(kind) => {
            StdCallable::RuntimeLimitConstructor(kind)
        }
        StdCallableSnapshot::TrustWrapper(wrapper) => StdCallable::TrustWrapper(wrapper),
        StdCallableSnapshot::Console(callable) => StdCallable::Console(callable),
        StdCallableSnapshot::Command(callable) => StdCallable::Command(callable),
        StdCallableSnapshot::Filesystem(callable) => StdCallable::Filesystem(callable),
        StdCallableSnapshot::Tcp(callable) => StdCallable::Tcp(callable),
        StdCallableSnapshot::Stream(callable) => StdCallable::Stream(callable),
        StdCallableSnapshot::Tls(callable) => StdCallable::Tls(callable),
        StdCallableSnapshot::Secret(callable) => StdCallable::Secret(callable),
        StdCallableSnapshot::Json(callable) => StdCallable::Json(callable),
        StdCallableSnapshot::Browser(callable) => StdCallable::Browser(callable),
    })
}
