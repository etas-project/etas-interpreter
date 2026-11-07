module app.main;

import std.task.{map_concurrent, run, Scheduler};

alias Schema = MemoryRegion<{ Entries: Store<string, string> }>;
let Memory = std.memory.region<Schema>(stable_id = "runtime_guards", store = "demo");

flow write(key: string) -> string {
    Memory.Entries.put(key, key);
    return key;
}

flow main(args: Array<string>) -> i32 {
    let values = run((scheduler: Scheduler) => {
        // a and b may overlap; the two writes to a acquire the same permit.
        return map_concurrent(["a", "b", "a"], write);
    });
    if values == ["a", "b", "a"] { return 0; }
    return 1;
}
