use super::super::*;
use crate::testing::allocation::{measure, measure_with_hard_limit};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn subprocess(case: &str, success: bool) {
    let name = std::thread::current().name().unwrap().to_owned();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env("ETAS_CONTAINER_RESOURCE_CASE", case)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "container resource probe exceeded 20s\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.success(),
        success,
        "{}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    #[cfg(unix)]
    if !success {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            output.status.signal(),
            Some(6),
            "budget probe must abort, not fail an assertion"
        );
    }
    if success {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
}

fn options(steps: u64) -> RunOptions {
    let mut options = RunOptions::default();
    options.execution_limits.max_steps = std::num::NonZeroU64::new(steps);
    options
}

fn source(end: &str) -> String {
    format!(
        r#"
module app.main;
flow main() -> u128 {{
    let all: Range<u128> = [0, {end});
    let window = all[3, 6);
    var sum: u128 = 0;
    for value in window limit Iterations(10) {{ sum = sum + value; }}
    for value in all limit Iterations(4294967295) {{ sum = sum + value; break; }}
    return sum;
}}
"#
    )
}

#[test]
fn huge_range_slice_and_early_break_obey_allocation_and_fuel_limits() {
    if std::env::var("ETAS_CONTAINER_RESOURCE_CASE").as_deref() != Ok("range") {
        return subprocess("range", true);
    }
    let small = checked_project(&source("32"));
    let huge = checked_project(&source("340282366920938463463374607431768211455"));
    let host = FakeHost::new(HostServiceAvailability::default());
    let run = |checked: &etas_frontend::CheckedProject, options| {
        crate::api::run_checked_blocking(
            checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            options,
        )
        .unwrap()
    };
    let (small_result, baseline) = measure(|| run(&small, options(512)));
    assert!(
        small_result.diagnostics.is_empty(),
        "{:?}",
        small_result.diagnostics
    );
    assert_eq!(
        small_result.value(),
        Some(&InterpValue::Number(crate::value::NumericValue::U128(12)))
    );
    let cap = baseline
        .bytes
        .checked_mul(2)
        .unwrap()
        .checked_add(4096)
        .unwrap();
    let (result, actual) = measure_with_hard_limit(cap, || run(&huge, options(512)));
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), small_result.value());
    assert!(actual.bytes <= cap);

    let no_break = checked_project(
        r#"
module app.main;
flow main() -> u128 {
    let all: Range<u128> = [0, 340282366920938463463374607431768211455);
    var sum: u128 = 0;
    for value in all limit Iterations(4294967295) { sum = sum + value; }
    return sum;
}
"#,
    );
    let (stopped, stopped_cost) = measure_with_hard_limit(cap, || run(&no_break, options(64)));
    assert!(stopped.value().is_none());
    assert!(
        stopped.diagnostics.iter().any(|d| d
            .message
            .contains("maximum interpreter execution steps (64)")),
        "{:?}",
        stopped.diagnostics
    );
    eprintln!(
        "bounded range: small={baseline:?}, huge={actual:?}, cap={cap}, no_break={stopped_cost:?}, fuel=512/64"
    );
}

#[test]
fn allocation_budget_probe_rejects_before_oversized_system_allocation() {
    if std::env::var("ETAS_CONTAINER_RESOURCE_CASE").as_deref() != Ok("allocator") {
        return subprocess("allocator", false);
    }
    // Without the guard this test exits successfully, which makes the parent
    // fail. The rejected request is only 8 KiB, not a machine-sized OOM probe.
    let _ = measure_with_hard_limit(1024, || std::hint::black_box(vec![1u8; 8192]));
}
