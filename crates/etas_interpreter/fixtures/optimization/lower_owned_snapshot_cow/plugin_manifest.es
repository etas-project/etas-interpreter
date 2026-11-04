module app.main;

flow main() -> Array<string> {
    let snapshot = ["name", "version", "capabilities", "permissions"];
    return snapshot.extend([]);
}
