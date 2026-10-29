use super::super::*;
use crate::api::codec::{checkpoint_artifact_json, checkpoint_from_json};

#[tokio::test(flavor = "current_thread")]
async fn implicit_spec_self_infers_method_rows_and_executes_each_callback_once_per_call() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
spec Runner { flow run<effect E>(self, callback: () -> unit ![E]) -> i32 ![E]; }
type Packet = { number: i32 };
impl Packet ~ Runner {
    flow run<effect F>(self, callback: () -> unit ![F]) -> i32 ![F] {
        callback(); callback(); return self.number;
    }
}
flow invoke<T ~ Runner>(value: T) -> i32 ![] {
    return value::Runner.run(() => { println("inferred")?; return; });
}
flow main() -> i32 ![] { return invoke(Packet { number = 21 }); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(21)));
    assert_eq!(host.stdout_text(), "inferred\ninferred\n");
    assert_eq!(host.console_call_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn implicit_spec_self_effectful_method_uses_checked_host_call_boundaries() {
    let checked = checked_project(
        r#"
module app.main;
import std.io.println;
spec Reporter { flow report(self) -> i32 ![Error<IOError>]; }
type Packet = { number: i32 };
impl Packet ~ Reporter {
    flow report(self) -> i32 ![Error<IOError>] { println("reported"); return self.number; }
}
flow main() -> i32 ![Error<IOError>] {
    return Packet { number = 7 }::Reporter.report();
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(7)));
    assert_eq!(host.stdout_text(), "reported\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn implicit_spec_self_dispatches_two_nominals_through_a_generic_wrapper() {
    let checked = checked_project(
        r#"
module app.main;
spec SnapshotEncoder { flow encode(self) -> i32; }
type Packet = { number: i32 };
type Snapshot = { number: i32 };
impl Packet ~ SnapshotEncoder { flow encode(self) -> i32 { return self.number; } }
impl Snapshot ~ SnapshotEncoder { flow encode(self) -> i32 { return self.number + 1; } }
flow encode<T ~ SnapshotEncoder>(value: T) -> i32 { return value::SnapshotEncoder.encode(); }
flow main() -> i32 { return encode(Packet { number = 7 }) + encode(Snapshot { number = 11 }); }
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value(), Some(&value::InterpValue::i32(19)));
}

#[tokio::test(flavor = "current_thread")]
async fn implicit_spec_self_method_checkpoint_replays_the_normal_call_boundary() {
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.checkpoint;
spec Encoder { flow encode(self) -> i32; }
type Packet = { number: i32 };
impl Packet ~ Encoder { flow encode(self) -> i32 { checkpoint("method"); return self.number; } }
flow encode<T ~ Encoder>(value: T) -> i32 { return value::Encoder.encode(); }
flow main() -> i32 { return encode(Packet { number = 7 }); }
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Checkpoint]));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first.value(), Some(&value::InterpValue::i32(7)));
    assert_eq!(first.checkpoints.len(), 1);
    let artifact =
        checkpoint_artifact_json(&["main.es".into()], "main", &first.checkpoints[0]).unwrap();
    let checkpoint = checkpoint_from_json(&artifact, &checked).unwrap();
    let resumed = Interpreter
        .resume_checkpoint(&checked, &checkpoint, &host, RunOptions::default())
        .await
        .unwrap();
    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value(), first.value());
}

#[tokio::test(flavor = "current_thread")]
async fn implicit_spec_self_preserves_generic_method_parameter_identity() {
    let checked = checked_project(
        r#"
module app.main;
spec Convert { flow convert<T>(self, value: T) -> Option<T>; }
type Packet = { number: i32 };
impl Packet ~ Convert { flow convert<U>(self, value: U) -> Option<U> { return Some(value); } }
flow main() -> string {
    let packet = Packet { number = 7 };
    let converted = packet::Convert.convert("done");
    return match converted { Some(value) => value, None => "bad" };
}

"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::String("done".into()))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn implicit_spec_self_remaps_swapped_generic_binder_names_without_collisions() {
    let checked = checked_project(
        r#"
module app.main;
spec Pair { flow pair<T, U>(self, first: T, second: U) -> (T, U); }
type Packet = { number: i32 };
impl Packet ~ Pair {
    flow pair<U, T>(self, first: U, second: T) -> (U, T) { return (first, second); }
}
flow main() -> string {
    let packet = Packet { number = 7 };
    let (text, number) = packet::Pair.pair("done", 7);
    return text;
}
"#,
    );
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(
        result.value(),
        Some(&value::InterpValue::String("done".into()))
    );
}
