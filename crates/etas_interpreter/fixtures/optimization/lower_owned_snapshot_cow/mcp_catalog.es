module app.main;

flow main() -> Array<string> {
    let snapshot = ["github", "filesystem", "search"];
    return snapshot.extend([]);
}
