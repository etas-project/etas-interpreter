module app.record_initialization_checkpoint;

import std.task.{run, Scheduler};
import std.runtime.checkpoint;

agent Counter = { n: i32 };
effect Wiring {
    action child() -> Counter;
    action ready() -> unit;
}
type Bundle = {
    @inject(Wiring.child)
    child: Counter,
    @inject(Wiring.ready)
    ready: unit,
};

flow main(args: Array<string>) -> i32 {
    let result = handle {
        run((scheduler: Scheduler) => {
            let bundle = Bundle {};
            return 0;
        })
    } with {
        Wiring.child() => {
            checkpoint("before-child");
            resume Counter { n = 5 };
        }
        Wiring.ready() => { finish 7; }
    };
    if result == 7 { return 0; }
    return 1;
}
