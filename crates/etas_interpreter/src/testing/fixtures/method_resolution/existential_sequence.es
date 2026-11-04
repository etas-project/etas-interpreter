module app.main;
import std.io.println;
import std.runtime.checkpoint;

spec Named { flow name(self) -> string ![]; }
type Label = string;
type Number = i32;
impl Label ~ Named {
    flow name(self) -> string ![] {
        checkpoint("first");
        println("first")?;
        return "first";
    }
}
impl Number ~ Named {
    flow name(self) -> string ![] {
        checkpoint("second");
        println("second")?;
        return "second";
    }
}
flow main() -> usize ![] {
    let values: Array<? ~ Named> = [Label("example"), Number(7)];
    let total: usize = 0;
    for value in values limit Iterations(2) {
        total = total + value.name().len();
    }
    return total;
}
