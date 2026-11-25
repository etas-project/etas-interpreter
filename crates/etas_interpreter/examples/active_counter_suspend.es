module app.active_counter_suspend;

import std.task.{run, start, await, Scheduler, Async};

effect Pause {
    action wait() -> unit;
}

agent Gate = {
    entered: bool,
    released: bool,
};

impl Gate {
    public flow enter(var self) -> unit ![] {
        self.entered = true;
    }

    public flow release(var self) -> unit ![] {
        self.released = true;
    }

    public flow is_entered(self) -> bool ![] {
        return self.entered;
    }

    public flow is_released(self) -> bool ![] {
        return self.released;
    }
}

agent Counter = {
    value: i32,
};

impl Counter {
    public flow add(var self, delta: i32) -> i32 ![] {
        self.value = self.value + delta;
        return self.value;
    }

    public flow update(var self, gate: Gate) -> i32 ![] {
        self.value = self.value + 1;
        gate.enter();
        while !gate.is_released() {
            perform Async.yield();
        }
        return self.value;
    }

    public flow get(self) -> i32 ![] {
        return self.value;
    }

    public flow pause(var self) -> i32 ![Pause] {
        self.value = 1;
        perform Pause.wait();
        return self.value;
    }
}

flow main(args: Array<string>) -> i32 {
    return run((scheduler: Scheduler) => {
        let counter = Counter { value = 0 };
        let alias = counter;
        let gate = Gate { entered = false, released = false };
        let update = start(() => counter.update(gate));
        while !gate.is_entered() {
            perform Async.yield();
        }
        let before = alias.get();
        gate.release();
        let reply = await(update);
        let after = alias.get();
        if before == 0 && reply == 1 && after == 1 {
            return 0;
        }
        return 1;
    });
}

flow finish_case(args: Array<string>) -> i32 {
    let counter = Counter { value = 0 };
    let stopped = handle {
        run((scheduler: Scheduler) => {
            let pending = start(() => counter.pause());
            return await(pending);
        })
    } with {
        Pause.wait() => { finish 7; }
    };
    if stopped == 7 && counter.get() == 0 {
        return 0;
    }
    return 1;
}

flow cancel_case(args: Array<string>) -> i32 {
    return run((scheduler: Scheduler) => {
        let counter = Counter { value = 0 };
        let gate = Gate { entered = false, released = false };
        let winner = std.task.race<i32, unit>([
            () => counter.update(gate),
            () => {
                while !gate.is_entered() {
                    perform Async.yield();
                }
                return 7;
            },
        ]);
        if winner == 7 && counter.get() == 0 {
            return 0;
        }
        return 1;
    });
}

flow queued_case(args: Array<string>) -> i32 {
    return run((scheduler: Scheduler) => {
        let counter = Counter { value = 0 };
        let gate = Gate { entered = false, released = false };
        let first = start(() => counter.update(gate));
        while !gate.is_entered() {
            perform Async.yield();
        }
        let second = start(() => counter.add(10));
        let before = counter.get();
        gate.release();
        let first_reply = await(first);
        let second_reply = await(second);
        if before == 0 && first_reply == 1 && second_reply == 11 && counter.get() == 11 {
            return 0;
        }
        return 1;
    });
}
