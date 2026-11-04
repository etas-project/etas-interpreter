module app.main;
import std.runtime.checkpoint;
import std.io.println;

spec Named {
    flow name(self, suffix: string) -> string ![Console.stdout_write];
}
type Label = string;

impl Label ~ Named {
    flow name(self, suffix: string) -> string ![Console.stdout_write] {
        println("method")?;
        checkpoint("method");
        return "label" + suffix;
    }
}

flow label() -> Label ![] {
    checkpoint("producer");
    return Label("example");
}

flow argument() -> string ![] {
    checkpoint("argument");
    return "!";
}

flow pack<T ~ Named>(value: T) -> ? ~ Named ![] {
    return value;
}

flow main() -> usize ![Console.stdout_write] {
    let value: ? ~ Named = label();
    checkpoint("packed");
    return value.name(argument()).len();
}
