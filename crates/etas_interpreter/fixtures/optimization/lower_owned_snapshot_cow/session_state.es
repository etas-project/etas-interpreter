module app.main;

flow main() -> List<string> {
    let snapshot = ["system"; "user"; "assistant"; "tool"];
    return snapshot.extend([]);
}
