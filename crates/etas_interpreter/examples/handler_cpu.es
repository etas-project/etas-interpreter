module app.main;

import std.task.{await, run, start, Scheduler};
import std.task.scheduler.park;

flow burn() -> i32 {
    var index = 0;
    while index < 100000 limit Iterations(100000) {
        index = index + 1;
    }
    return 1;
}

// The Handler transfers continuations to the coordinator's scheduler.
// Only burn() executes on CPU workers.
flow handler_work() -> i32 {
    return run((scheduler: Scheduler) => {
        let first = start(burn);
        let second = start(burn);
        let third = start(burn);
        let fourth = start(burn);
        let a = handle { await(first) } with {
            Async.await(waited) as k => { park(scheduler, waited, k); }
        };
        let b = handle { await(second) } with {
            Async.await(waited) as k => { park(scheduler, waited, k); }
        };
        let c = handle { await(third) } with {
            Async.await(waited) as k => { park(scheduler, waited, k); }
        };
        let d = handle { await(fourth) } with {
            Async.await(waited) as k => { park(scheduler, waited, k); }
        };
        return a + b + c + d;
    });
}

flow main(args: Array<string>) -> i32 {
    if handler_work() == 4 { return 0; }
    return 1;
}
