module app.main;

flow main() -> Array<string> {
    let snapshot = ["src", "tests", "docs", "Cargo.toml"];
    return snapshot.extend([]);
}
