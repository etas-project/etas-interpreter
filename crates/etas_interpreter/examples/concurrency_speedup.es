module app.main;

import std.task.{map_concurrent, run, Scheduler};

// Compare the same program with --cpu-threads 1 and --cpu-threads 4.
flow burn(value: i32) -> i32 {
    var index = 0;
    while index < 100000 limit Iterations(100000) {
        index = index + 1;
    }
    return value;
}

flow main(args: Array<string>) -> i32 {
    let values = run((scheduler: Scheduler) => {
        return map_concurrent([1, 2, 3, 4], burn);
    });
    if values == [1, 2, 3, 4] { return 0; }
    return 1;
}
