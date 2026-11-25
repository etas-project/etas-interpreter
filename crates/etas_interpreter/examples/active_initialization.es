module app.active_initialization;

import std.task.{run, Scheduler};

agent Counter = { n: i32 };
impl Counter {
    public flow get(self) -> i32 ![] {
        return self.n;
    }
    public flow increment(var self) -> i32 ![] {
        self.n = self.n + 1;
        return self.n;
    }
}

effect Wiring {
    action child() -> Counter;
    action ready() -> unit;
}

agent Family = {
    borrowed: Counter,
    @inject(Wiring.child)
    child: Counter,
    @inject(Wiring.ready)
    ready: unit,
};
impl Family {
    public flow total(self) -> i32 {
        return self.borrowed.get() + self.child.get();
    }
}

flow main(args: Array<string>) -> i32 {
    let existing = Counter { n = 10 };
    var created = 0;
    let interrupted = handle {
        let family = Family { borrowed = existing };
        0
    } with {
        Wiring.child() => {
            created = created + 1;
            resume run((scheduler: Scheduler) => Counter { n = 20 });
        }
        Wiring.ready() => { finish 7; }
    };
    let still_owned = existing.increment();
    let complete = handle {
        Family { borrowed = existing }
    } with {
        Wiring.child() => {
            created = created + 1;
            resume run((scheduler: Scheduler) => Counter { n = 30 });
        }
        Wiring.ready() => { resume (); }
    };
    if interrupted == 7 && created == 2 && still_owned == 11
        && existing.get() == 11 && complete.total() == 41 {
        return 0;
    }
    return 1;
}
