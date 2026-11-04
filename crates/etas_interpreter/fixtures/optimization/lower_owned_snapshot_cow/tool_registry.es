module app.main;

flow main() -> Array<string> {
    let snapshot = ["read", "write", "shell", "web"];
    return snapshot.extend([]);
}
