module app.main;
import std.io.println;
import std.runtime.checkpoint;

type Seed = { value: i32 };
type End = { value: i32 };
spec Number { flow number(self) -> i32; }
impl Seed {
    flow advance(self, left: i32, right: i32) -> End {
        println("advance");
        return End { value = self.value + left + right };
    }
}
impl End ~ Number {
    flow number(self) -> i32 { println("number"); return self.value; }
}
flow receiver() -> Seed {
    println("receiver");
    checkpoint("receiver");
    return Seed { value = 10 };
}
flow argument(label: string, value: i32) -> i32 {
    println(label);
    checkpoint(label);
    return value;
}
flow main() -> i32 {
    return receiver().advance(right = argument("right", 12), left = argument("left", 8)).number();
}
