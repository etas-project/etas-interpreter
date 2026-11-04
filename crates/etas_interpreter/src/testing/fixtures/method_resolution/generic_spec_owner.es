module app.main;
import std.io.println;
import std.runtime.checkpoint;

effect Probe { action hit<T>() -> unit; }
spec Read<T> { flow read(self) -> T ![Probe.hit<T>]; }
type Container<T> = { value: T };
impl Container<T> ~ Read<T> {
    public flow read(self) -> T ![Probe.hit<T>] {
        perform Probe.hit<T>();
        println("read")?;
        checkpoint("read");
        return self.value;
    }
}

flow load<S ~ Read<i32>>(value: S) -> i32 ![Probe.hit<i32>] {
    return value.read();
}

flow main() -> i32 ![] {
    return handle {
        return load(Container<i32> { value = 7 });
    } with {
        Probe.hit<i32>() => {
            checkpoint("probe");
            resume ();
        }
    };
}
