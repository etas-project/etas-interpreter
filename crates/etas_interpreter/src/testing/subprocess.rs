pub(crate) fn in_subprocess(worker: &str) -> bool {
    if std::env::var_os(worker).is_some() {
        return false;
    }
    let thread = std::thread::current();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", thread.name().unwrap(), "--nocapture"])
        .env(worker, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "projection subprocess: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    true
}
