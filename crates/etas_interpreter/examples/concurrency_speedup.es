module app.main;

import std.io.println;
import std.task.{join, run, Scheduler};

flow serial_work() -> i32 {
    println("job-1")?;
    println("job-2")?;
    println("job-3")?;
    println("job-4")?;
    return 4;
}

flow concurrent_work() -> i32 {
    return run((scheduler: Scheduler) => {
        let (a, b, c, d) = join((
            () => { println("job-1")?; return 1; },
            () => { println("job-2")?; return 1; },
            () => { println("job-3")?; return 1; },
            () => { println("job-4")?; return 1; }
        ));
        return a + b + c + d;
    });
}

flow main(use_concurrency: bool) -> i32 {
    if use_concurrency {
        return concurrent_work();
    }
    return serial_work();
}
