use super::super::*;
use crate::testing::host::TestMemoryBackend;

async fn exercise_paging(storage: TestMemoryBackend, check_keys: bool) {
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.storage = Some(storage);
    let writes = (0..101)
        .map(|i| format!("ProjectMemory.Papers.put(\"key-{i:03}\", \"value\");"))
        .collect::<Vec<_>>()
        .join("\n");
    let (body, count) = if check_keys {
        (
            format!("{writes}\nreturn ProjectMemory.Papers.keys();"),
            101,
        )
    } else {
        (
            format!("{writes}\nProjectMemory.Papers.clear(); return ProjectMemory.Papers.keys();"),
            0,
        )
    };
    let checked = checked_project(&format!(
        r#"
module app.main;
alias ProjectMemorySchema = MemoryRegion<{{ Papers: Store<string, string> }}>;
let ProjectMemory = std.memory.region<ProjectMemorySchema>(stable_id = "paging", store = "test");
flow main() -> List<string> {{ {body} }}
"#
    ));
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
    let Some(InterpValue::List(keys)) = result.value() else {
        panic!("expected key list")
    };
    assert_eq!(keys.borrow().len(), count);
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_keys_and_clear_volatile() {
    for check_keys in [true, false] {
        exercise_paging(
            TestMemoryBackend::Volatile(etas_host::InMemoryMemoryClient::new()),
            check_keys,
        )
        .await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_keys_and_clear_sqlite() {
    let root = std::env::temp_dir().join(format!(
        "etas-memory-paging-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for check_keys in [true, false] {
        exercise_paging(
            TestMemoryBackend::Sqlite(
                etas_host::SqliteMemoryClient::open(root.join(format!("memory-{check_keys}.db")))
                    .unwrap(),
            ),
            check_keys,
        )
        .await;
    }
    std::fs::remove_dir_all(root).unwrap();
}

async fn exercise_typed_pages(storage: TestMemoryBackend) {
    let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
    host.storage = Some(storage);
    let writes = (0..101)
        .map(|i| format!("ProjectMemory.Papers.put(\"key-{i:03}\", \"value\");"))
        .collect::<Vec<_>>()
        .join("\n");
    let checked = checked_project(&format!(
        r#"
module app.main;
import std.memory.{{page, get_entry}};
alias ProjectMemorySchema = MemoryRegion<{{ Papers: Store<string, string> }}>;
let ProjectMemory = std.memory.region<ProjectMemorySchema>(stable_id = "typed-paging", store = "test");
flow main() -> List<string> {{
    {writes}
    let first = ProjectMemory.Papers.page(None(), 50);
    let second = page(ProjectMemory.Papers, first.cursor, 50);
    let third = ProjectMemory.Papers.page(second.cursor, 50);
    var keys: List<string> = [];
    for entry in first.entries limit Iterations(50) {{ keys = keys.push(entry.key); }}
    for entry in second.entries limit Iterations(50) {{ keys = keys.push(entry.key); }}
    for entry in third.entries limit Iterations(50) {{ keys = keys.push(entry.key); }}
    match third.cursor {{ Some(_) => {{ return []; }}, None => {{}} }}
    match ProjectMemory.Papers.get_entry("key-000") {{
        Some(entry) => {{ ProjectMemory.Papers.put_versioned(entry.key, "updated", entry.version); }},
        None => {{ return []; }}
    }}
    match get_entry(ProjectMemory.Papers, "key-000") {{
        Some(entry) => {{ keys = keys.push(entry.value); }},
        None => {{ return []; }}
    }}
    return keys;
}}
"#
    ));
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
    let Some(InterpValue::List(keys)) = result.value() else {
        panic!("expected typed key list")
    };
    let keys = keys.borrow();
    assert_eq!(keys.len(), 102);
    for (i, key) in (0..101).rev().zip(keys.iter().skip(1)) {
        assert_eq!(key, &InterpValue::String(format!("key-{i:03}")));
    }
    assert_eq!(keys[0], InterpValue::String("updated".into()));
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_typed_page_and_versioned_entry_volatile() {
    exercise_typed_pages(TestMemoryBackend::Volatile(
        etas_host::InMemoryMemoryClient::new(),
    ))
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_typed_page_and_versioned_entry_sqlite() {
    let path = std::env::temp_dir().join(format!(
        "etas-typed-pages-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    exercise_typed_pages(TestMemoryBackend::Sqlite(
        etas_host::SqliteMemoryClient::open(&path).unwrap(),
    ))
    .await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_qualified_std_calls_preserve_checked_dispatch() {
    let checked = checked_project(
        r#"
module app.main;
alias Schema = MemoryRegion<{ Entries: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "qualified-paging", store = "test");
flow main() -> string {
    Memory.Entries.put("key", "value");
    let page = std.memory.page(Memory.Entries, None(), 1);
    match page.cursor { Some(_) => { return "unexpected cursor"; }, None => {} }
    match std.memory.get_entry(Memory.Entries, "key") {
        Some(entry) => { return entry.value; },
        None => { return "missing entry"; }
    }
}
"#,
    );
    let host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
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
    assert_eq!(result.value(), Some(&InterpValue::String("value".into())));
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_rejects_stale_foreign_and_zero_limit_cursors() {
    for sqlite in [false, true] {
        let workspace = etas_host::TestWorkspace::create("typed-cursor-rejection").unwrap();
        for (case, tail, expected) in [
            (
                "foreign",
                "let other = ProjectMemory.Other.page(first.cursor, 1);",
                "another Store",
            ),
            (
                "stale",
                "ProjectMemory.Papers.put(\"c\", \"new\"); let next = ProjectMemory.Papers.page(first.cursor, 1);",
                "stale",
            ),
            (
                "zero",
                "let next = ProjectMemory.Papers.page(first.cursor, 0);",
                "positive u32",
            ),
        ] {
            let mut host = FakeHost::new(availability(&[HostRequirementKind::DurableMemory]));
            host.storage = Some(if sqlite {
                TestMemoryBackend::Sqlite(
                    etas_host::SqliteMemoryClient::open(
                        workspace.path().join(format!("{case}.db")),
                    )
                    .unwrap(),
                )
            } else {
                TestMemoryBackend::Volatile(etas_host::InMemoryMemoryClient::new())
            });
            let checked = checked_project(&format!(
                r#"
module app.main;
alias Schema = MemoryRegion<{{ Papers: Store<string, string>, Other: Store<string, string> }}>;
let ProjectMemory = std.memory.region<Schema>(stable_id = "cursor-reject", store = "test");
flow main() -> unit {{
    ProjectMemory.Papers.put("a", "one");
    ProjectMemory.Papers.put("b", "two");
    ProjectMemory.Other.put("a", "other");
    let first = ProjectMemory.Papers.page(None(), 1);
    {tail}
}}
"#
            ));
            let result = Interpreter
                .run_checked(
                    &checked,
                    EntryPoint {
                        item: checked.entry.unwrap(),
                    },
                    vec![],
                    &host,
                    RunOptions::default(),
                )
                .await
                .unwrap();
            assert!(
                result
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "{case} sqlite={sqlite}: {:?}",
                result.diagnostics
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn memory_paging_checkpoint_preserves_checked_result_type_and_cursor() {
    let workspace = etas_host::TestWorkspace::create("typed-page-checkpoint").unwrap();
    let path = workspace.path().join("store.db");
    let checked = checked_project(
        r#"
module app.main;
import std.runtime.{checkpoint};
alias Schema = MemoryRegion<{ Papers: Store<string, string> }>;
let ProjectMemory = std.memory.region<Schema>(stable_id = "typed-page-checkpoint", store = "test");
flow cursor() -> Option<MemoryCursor> { checkpoint("arguments"); return None(); }
flow main() -> MemoryPage<string, string> {
    ProjectMemory.Papers.put("a", "one");
    ProjectMemory.Papers.put("b", "two");
    let first = ProjectMemory.Papers.page(cursor(), 1);
    checkpoint("page");
    return ProjectMemory.Papers.page(first.cursor, 1);
}
"#,
    );
    let make_host = || {
        let mut host = FakeHost::new(availability(&[
            HostRequirementKind::DurableMemory,
            HostRequirementKind::Checkpoint,
        ]));
        host.storage = Some(TestMemoryBackend::Sqlite(
            etas_host::SqliteMemoryClient::open(&path).unwrap(),
        ));
        host
    };
    let host = make_host();
    let result = Interpreter
        .run_checked(
            &checked,
            EntryPoint {
                item: checked.entry.unwrap(),
            },
            vec![],
            &host,
            RunOptions::default(),
        )
        .await
        .unwrap();
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    assert_eq!(result.checkpoints.len(), 2);
    for checkpoint in &result.checkpoints {
        let artifact =
            crate::api::codec::checkpoint_artifact_json(&["page.es".into()], "main", checkpoint)
                .unwrap();
        let decoded = crate::api::codec::checkpoint_from_json(&artifact, &checked).unwrap();
        let mut obsolete = artifact;
        obsolete["schema"] = "etas.cli.interpreter-checkpoint.v21".into();
        assert!(
            crate::api::codec::checkpoint_from_json(&obsolete, &checked)
                .unwrap_err()
                .message()
                .contains("unsupported checkpoint artifact schema")
        );
        let resumed = Interpreter
            .resume_checkpoint(&checked, &decoded, &make_host(), RunOptions::default())
            .await
            .unwrap();
        assert!(resumed.diagnostics.is_empty(), "{:?}", resumed.diagnostics);
        assert_eq!(resumed.value(), result.value());
    }
}
