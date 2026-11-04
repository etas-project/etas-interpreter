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
    public flow replace<T>(self, value: T) -> Container<T> ![] {
        let original = self.copy();
        checkpoint("replace");
        return Container<T> { value };
    }
    public flow get(self) -> T ![] { return self.value; }
}

flow main() -> string ![] {
    let original = Container<i32> { value = 7 };
    let changed = original.copy().replace("changed");
    if original.get() != 7 { return "mutated"; }
    return changed.get();
}
