module app.main;
import std.io.println;
import std.runtime.checkpoint;

spec Read<T> { flow read(self) -> T ![]; }
type Container<T> = { value: T };
impl Container<T> ~ Read<T> {
    public flow read(self) -> T ![] {
        println("read")?;
        checkpoint("read");
        return self.value;
    }
}

flow load<S ~ Read<i32>>(value: S) -> i32 ![] {
    return value.read();
}

flow main() -> i32 ![] {
    return load(Container<i32> { value = 7 });
}
