module app.main;
import std.runtime.checkpoint;

spec Read<T> { flow read(self) -> T ![]; }
type Container<T> = { value: T };
impl Container<T> ~ Read<T> {
    public flow read(self) -> T ![] {
        checkpoint("method");
        return self.value;
    }
}
flow load<S ~ Read<i32>>(value: S) -> i32 ![] { return value.read(); }
flow target() -> i32 ![] { return load(Container<i32> { value = 7 }); }
flow inspect() -> unit { let definition = ^target; return; }
flow main(current: () -> i32 ![]) -> i32 ![] { return current(); }
