module app.main;
import std.io.println;

type Seed = { number: i32 };
type End = { number: i32 };

flow receiver() -> Seed ![] {
    println("receiver")?;
    return Seed { number = 3 };
}
flow argument(label: string, number: i32) -> i32 ![] {
    println(label)?;
    return number;
}
impl Seed {
    public flow advance(self, left: i32, right: i32) -> End ![] {
        println("advance")?;
        return End { number = self.number + left * 10 + right };
    }
}
spec Number { flow number(self) -> i32 ![]; }
impl End ~ Number {
    flow number(self) -> i32 ![] {
        println("number")?;
        return self.number;
    }
}
flow main() -> i32 ![] {
    return receiver().advance(right = argument("right", 7), left = argument("left", 2)).number();
}
