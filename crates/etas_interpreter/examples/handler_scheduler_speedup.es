module app.main;

import std.io.println;
import std.task.{await, run, start, Scheduler};
import std.task.scheduler.park;

flow serial_work() -> i32 {
    println("job-1")?;
    println("job-2")?;
    println("job-3")?;
    println("job-4")?;
    return 4;
}

flow handler_scheduled_work() -> i32 {
    return run((scheduler: Scheduler) => {
        let first = start(() => { println("job-1")?; return 1; });
        let second = start(() => { println("job-2")?; return 1; });
        let third = start(() => { println("job-3")?; return 1; });
        let fourth = start(() => { println("job-4")?; return 1; });

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

flow main(use_handler_scheduler: bool) -> i32 {
    if use_handler_scheduler {
        return handler_scheduled_work();
    }
    return serial_work();
}
