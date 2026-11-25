module app.main;
import std.agent.Autonomous;

agent Worker = { n: i32 };
impl Worker ~ Autonomous {
    flow main(var self) -> unit {
        self.n = self.n + 1;
        return;
    }
}
impl Worker {
    public flow get(self) -> i32 ![] {
        return self.n;
    }
    public flow barrier(var self) -> i32 ![] {
        return self.n;
    }
}
flow main(args: Array<string>) -> i32 {
    let worker = Worker { n = 41 };
    let alias = worker;
    let read = alias.get;
    if worker.barrier() == 42 && read() == 42 {
        return 0;
    }
    return 1;
}
