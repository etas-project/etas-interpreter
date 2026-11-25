module app.active_injection;

effect Supply {
    action first() -> i32;
    action value<T>() -> T;
}

type Settings = {
    @inject(Supply.first)
    first: i32,
    explicit: i32,
    @inject(Supply.first)
    second: i32,
};

type Box<T> = {
    marker: T,
    @inject(Supply.value<T>)
    value: T,
};

alias IntBox = Box<i32>;

agent Counter = {
    @inject(Supply.first)
    n: i32,
};

impl Counter {
    public flow get(self) -> i32 ![] {
        return self.n;
    }

    public flow increment(var self) -> i32 ![] {
        self.n = self.n + 1;
        return self.n;
    }
}

flow main(args: Array<string>) -> i32 {
    var order = "";
    var calls = 0;
    let settings = handle {
        Settings {
            explicit = { order = order + "E"; 9 },
        }
    } with {
        Supply.first() => {
            calls = calls + 1;
            if calls == 1 { order = order + "A"; }
            else { order = order + "B"; }
            resume calls;
        }
    };

    let first = 11;
    let second = 22;
    let explicit = 33;
    let supplied = Settings { second, first = first, explicit };
    let number = handle {
        Box { marker = 3 }
    } with {
        Supply.value<i32>() => { resume 7; }
    };
    let text = handle {
        Box { marker = "marker" }
    } with {
        Supply.value<string>() => { resume "injected"; }
    };
    let alias_value = handle {
        IntBox { marker = 8 }
    } with {
        Supply.value<i32>() => { resume 9; }
    };
    let counter = handle {
        Counter {}
    } with {
        Supply.first() => { resume 41; }
    };

    if order == "EAB" && calls == 2
        && settings.first == 1 && settings.second == 2 && settings.explicit == 9
        && supplied.first == 11 && supplied.second == 22
        && number.value == 7 && text.value == "injected" && alias_value.value == 9
        && counter.get() == 41 && counter.increment() == 42 && counter.get() == 42 {
        return 0;
    }
    return 1;
}
