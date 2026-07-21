use super::super::*;

#[tokio::test(flavor = "current_thread")]
async fn effectful_return_crosses_if_handle_and_console_suspension() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;

flow effectful_success() -> i32 ![Console, Error<IOError>] {
    println("success");
    return 0;
}

flow fallback() -> i32 ![Console, Error<IOError>] {
    println("usage");
    return 1;
}

flow main(args: Array<string>) -> i32 ![Console, Error<IOError>] {
    return handle {
        let condition = args[0] == "fetch";
        if condition {
            return effectful_success();
        }
        return fallback();
    } with {
        Error<IndexError>.raise(_) => {
            finish fallback();
        }
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::Array(value::ArrayValue::new(vec![
                value::InterpValue::String("fetch".to_owned()),
            ]))],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
    assert_eq!(host.stdout_text(), "success\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn pure_return_from_if_skips_fallback() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
    if true {
        return 0;
    }
    return 1;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
}

#[tokio::test(flavor = "current_thread")]
async fn return_after_perform_resume_skips_fallback() {
    let checked = checked_project(
        r#"
module app.main;

effect Gate {
    action allow() -> bool;
}

flow main() -> i32 {
    return handle {
        if perform Gate.allow() {
            return 0;
        }
        return 1;
    } with {
        Gate.allow() => {
            resume true;
        }
    };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
}

#[tokio::test(flavor = "current_thread")]
async fn return_crosses_untriggered_handle_boundary() {
    let checked = checked_project(
        r#"
module app.main;

effect Gate {
    action request() -> i32;
}

flow main() -> i32 {
    return handle {
        if true {
            return 0;
        }
        return perform Gate.request();
    } with {
        Gate.request() => {
            resume 1;
        }
    };
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
}

#[tokio::test(flavor = "current_thread")]
async fn handler_finish_returns_effectful_fallback_once() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;

flow fallback() -> i32 ![Console, Error<IOError>] {
    println("usage");
    return 1;
}

flow main(args: Array<string>) -> i32 ![Console, Error<IOError>] {
    return handle {
        let _command = args[0];
        return 0;
    } with {
        Error<IndexError>.raise(_) => {
            finish fallback();
        }
    };
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::Array(
                value::ArrayValue::new(Vec::new()),
            )],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(1)));
    assert_eq!(host.stdout_text(), "usage\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn effectful_return_from_match_arm_skips_fallback() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;

flow effectful_success() -> i32 ![Console, Error<IOError>] {
    println("success");
    return 0;
}

flow fallback() -> i32 ![Console, Error<IOError>] {
    println("usage");
    return 1;
}

flow main(command: string) -> i32 ![Console, Error<IOError>] {
    match command {
        "fetch" => {
            return effectful_success();
        }
        _ => {
            return fallback();
        }
    }
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            vec![value::InterpValue::String("fetch".to_owned())],
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
    assert_eq!(host.stdout_text(), "success\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn return_crosses_nested_blocks_and_if_statements() {
    let checked = checked_project(
        r#"
module app.main;

flow main() -> i32 {
    if true {
        if true {
            if true {
                return 0;
            }
        }
    }
    return 1;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
}

#[tokio::test(flavor = "current_thread")]
async fn return_crosses_retry_after_console_suspension() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;
import std.runtime.limits.Attempts;

flow effectful_success() -> i32 ![Console, Error<IOError>] {
    println("success");
    return 0;
}

flow fallback() -> i32 ![Console, Error<IOError>] {
    println("usage");
    return 1;
}

flow main() -> i32 ![Console, Error<IOError>] {
    retry limit Attempts(2) {
        return effectful_success();
    }
    return fallback();
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::Console]));

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
    assert_eq!(host.stdout_text(), "success\n");
    assert_eq!(host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn checkpoint_resume_preserves_pending_return_continuation() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;
import std.runtime.checkpoint;

flow effectful_success() -> i32 ![Console, Error<IOError>] {
    checkpoint("before-success");
    println("success");
    return 0;
}

flow fallback() -> i32 ![Console, Error<IOError>] {
    println("usage");
    return 1;
}

flow main() -> i32 ![Console, Error<IOError>] {
    if true {
        return effectful_success();
    }
    return fallback();
}
"#,
    );
    let requirements = [
        HostRequirementKind::Checkpoint,
        HostRequirementKind::Console,
    ];
    let first_host = FakeHost::new(availability(&requirements));
    let first = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &first_host,
            RunOptions::default(),
        )
        .await;

    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    let checkpoint = first.checkpoints.first().expect("checkpoint record");
    let resumed_host = FakeHost::new(availability(&requirements));
    let resumed = Interpreter
        .resume_checkpoint(&checked, checkpoint, &resumed_host, RunOptions::default())
        .await;

    assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
    assert_eq!(resumed.value, Some(value::InterpValue::i32(0)));
    assert_eq!(resumed_host.stdout_text(), "success\n");
    assert_eq!(resumed_host.console_call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn call_boundary_consumes_callee_return_but_preserves_caller_return() {
    let checked = checked_project(
        r#"
module app.main;

flow callee() -> i32 {
    return 7;
}

flow caller() -> i32 {
    return callee() + 1;
}

flow main() -> i32 {
    if true {
        return caller();
    }
    return 99;
}
"#,
    );

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &FakeHost::new(HostServiceAvailability::default()),
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(8)));
}

#[tokio::test(flavor = "current_thread")]
async fn return_across_suspended_handle_removes_handler_before_checkpoint() {
    let checked = checked_project(
        r#"
module app.main;
import std.effects.Console;
import std.io.println;
import std.runtime.checkpoint;

effect Gate {
    action request() -> i32;
}

flow effectful_success() -> i32 ![Console, Error<IOError>] {
    println("success");
    return 0;
}

flow handled() -> i32 ![Console, Error<IOError>] {
    return handle {
        if true {
            return effectful_success();
        }
        return perform Gate.request();
    } with {
        Gate.request() => {
            resume 1;
        }
    };
}

flow main() -> i32 ![Console, Error<IOError>] {
    let result = handled();
    checkpoint("after-handle");
    return result;
}
"#,
    );
    let host = FakeHost::new(availability(&[
        HostRequirementKind::Checkpoint,
        HostRequirementKind::Console,
    ]));

    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.expect("entry item"),
            },
            Vec::new(),
            &host,
            RunOptions::default(),
        )
        .await;

    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.value, Some(value::InterpValue::i32(0)));
    assert_eq!(host.stdout_text(), "success\n");
    let checkpoint = result.checkpoints.first().expect("checkpoint record");
    assert!(
        checkpoint.handlers.handlers.is_empty(),
        "completed handle leaked into checkpoint: {:?}",
        checkpoint.handlers.handlers
    );
}
