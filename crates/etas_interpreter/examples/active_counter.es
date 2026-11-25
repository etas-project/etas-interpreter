module app.active_counter;

agent Counter = {
    value: i32,
};

impl Counter {
    public flow add(var self, delta: i32) -> i32 ![] {
        self.increase(delta);
        return self.get();
    }

    flow increase(var self, delta: i32) -> unit ![] {
        self.value = self.value + delta;
    }

    public flow get(self) -> i32 ![] {
        return self.value;
    }

    public flow reject(var self) -> Result<i32, string> ![] {
        self.value = self.value + 2;
        return Err("returned error value");
    }
}

flow main(args: Array<string>) -> i32 ![] {
    let counter = Counter { value = 0 };
    let alias = counter;
    let read = alias.get;
    let update = alias.add;
    let other = Counter { value = 99 };
    update(1);
    let first = read();
    let result = counter.reject();
    if first == 1 && read() == 3 && other.get() == 99 {
        return 0;
    }
    return 1;
}
