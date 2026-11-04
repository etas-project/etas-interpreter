module app.main;
import std.io.println;
import std.runtime.checkpoint;

type Container<T> = { value: T };
impl Container<T> {
    public flow copy(self) -> Container<T> ![] {
        println("copy")?;
        checkpoint("copy");
        return Container<T> { value = self.value };
    }
    public flow capture<T>(self, other: T) -> (() -> T ![]) ![] {
        return () => {
            checkpoint("captured");
            let original = self.copy();
            return other;
        };
    }
    public flow wrap(self) -> (() -> string ![]) ![] {
        return self.capture("changed");
    }
}

flow main() -> string ![] {
    let callback = Container<i32> { value = 7 }.wrap();
    checkpoint("before");
    return callback();
}
