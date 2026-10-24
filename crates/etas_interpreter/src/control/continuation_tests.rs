use super::Continuation;
use crate::testing::allocation::measure;

fn worker(key: &str) -> bool {
    if std::env::var_os(key).is_some() {
        return false;
    }
    let thread = std::thread::current();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", thread.name().unwrap(), "--nocapture"])
        .env(key, "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&result.stderr));
    true
}

fn chain(depth: usize, mode: usize) -> Continuation {
    (0..depth).fold(Continuation::Return, |node, _| match mode {
        0 => Continuation::CallBoundary { outer: node.into() },
        1 => Continuation::HandlerDispatch { outer: node.into() },
        _ => Continuation::Chain {
            inner: node.into(),
            outer: Continuation::Finish.into(),
        },
    })
}

#[test]
fn runtime_continuation_clone_shares_structure_without_recursive_copy() {
    if worker("ETAS_RUNTIME_CONTINUATION_CLONE_WORKER") {
        return;
    }
    for depth in [64, 1000, 4000, 30_000] {
        for mode in 0..3 {
            let root = chain(depth, mode);
            let (alias, cost) = measure(|| root.clone());
            eprintln!("runtime continuation clone depth={depth} mode={mode}: {cost:?}");
            assert_eq!(cost.count, 0, "recursive structural copy: {cost:?}");
            let mut cursor = &alias;
            for _ in 0..depth {
                cursor = match cursor {
                    Continuation::CallBoundary { outer }
                    | Continuation::HandlerDispatch { outer } => outer,
                    Continuation::Chain { inner, outer } => {
                        assert!(matches!(**outer, Continuation::Finish));
                        inner
                    }
                    _ => panic!("wrong continuation shape"),
                };
            }
            assert!(matches!(cursor, Continuation::Return));
            drop(root);
            drop(alias);
        }
    }
}

#[test]
fn runtime_continuation_drop_is_stack_safe_without_a_test_release_guard() {
    if worker("ETAS_RUNTIME_CONTINUATION_DROP_WORKER") {
        return;
    }
    for mode in 0..3 {
        let (_, cost) = measure(|| drop(chain(30_000, mode)));
        assert_eq!(
            cost.bytes, cost.released_bytes,
            "continuation leaked: {cost:?}"
        );
    }
}

#[test]
fn runtime_continuation_shared_dag_releases_each_backing_once() {
    if worker("ETAS_RUNTIME_CONTINUATION_DAG_WORKER") {
        return;
    }
    let depth = 30_000;
    let (_, cost) = measure(|| {
        let mut root = Continuation::Return;
        for _ in 0..depth {
            let child: super::ContinuationLink = root.into();
            root = Continuation::Chain {
                inner: child.clone(),
                outer: child,
            };
        }
        let alias = root.clone();
        drop(root);
        drop(alias);
    });
    assert_eq!(cost.count, depth);
    assert_eq!(
        cost.bytes,
        depth * (size_of::<Continuation>() + 2 * size_of::<usize>())
    );
    assert_eq!(cost.bytes, cost.released_bytes, "DAG leaked: {cost:?}");
}

#[test]
fn runtime_continuation_cow_detaches_only_changed_control_path() {
    let mut link: super::ContinuationLink = Continuation::Chain {
        inner: Continuation::Return.into(),
        outer: Continuation::Finish.into(),
    }
    .into();
    let retained = link.clone();
    let (_, cost) = measure(|| {
        let Continuation::Chain { inner, .. } = link.as_mut() else {
            panic!("chain")
        };
        **inner = Continuation::Resume;
    });
    assert_eq!(cost.count, 2, "root and changed child only: {cost:?}");
    let (
        Continuation::Chain {
            inner: old,
            outer: old_outer,
        },
        Continuation::Chain {
            inner: new,
            outer: new_outer,
        },
    ) = (&*retained, &*link)
    else {
        panic!("chain")
    };
    assert!(matches!(**old, Continuation::Return));
    assert!(matches!(**new, Continuation::Resume));
    assert!(std::ptr::eq(&**old_outer, &**new_outer));

    let (owned, cost) = measure(|| link.into_value());
    assert_eq!(cost.count, 0, "unique extraction copied: {cost:?}");
    let second = retained.clone();
    let (shared, cost) = measure(|| retained.into_value());
    assert_eq!(
        cost.count, 0,
        "shared extraction rebuilt descendants: {cost:?}"
    );
    drop((owned, shared, second));
}
