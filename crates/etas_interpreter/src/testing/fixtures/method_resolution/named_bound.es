module app.main;

type Counter = { value: i32 };
impl Counter { flow copy(self) -> Counter { return self; } }
spec Add { flow add(self, amount: i32) -> i32; }
impl Counter ~ Add {
    flow add(self, input: i32) -> i32 { return self.value + input; }
}
flow bounded<T ~ Add>(value: T) -> i32 { return value.add(amount = 2); }
flow selected<T ~ Add>(value: T) -> i32 { return value::Add.add(amount = 3); }
flow main() -> i32 {
    let value = Counter { value = 4 };
    return bounded(value) + selected(value) + value.add(amount = 5);
}
