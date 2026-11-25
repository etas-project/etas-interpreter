module app.main;
import std.agent.Autonomous;
import std.io.{println, read_line};
effect Startup { action ready() -> unit; }
agent Worker = { n: i32 };
impl Worker ~ Autonomous {
    flow main(var self) -> unit {
        self.n = self.n + 1;
        perform Startup.ready();
        println("pending");
        return;
    }
}
impl Worker {
    public flow get(self) -> i32 ![] { return self.n; }
    public flow barrier(var self) -> i32 ![] { return self.n; }
}
public flow main_resume() -> i32 {
    return handle {
        let worker = Worker { n = 41 };
        let alias = worker;
        let before = alias.get();
        let after = worker.barrier();
        before * 100 + after + alias.get()
    } with { Startup.ready() => resume (); };
}

public flow main_stop() -> i32 {
    return handle {
        let worker = Worker { n = 41 };
        read_line();
        worker.get()
    } with { Startup.ready() => resume (); };
}

flow main(args: Array<string>) -> i32 {
    if args.is_empty() {
        return main_resume();
    }
    return main_stop();
}
