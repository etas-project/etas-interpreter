module app.main;
import std.compiler.{compile, CompileError};

agent Worker() -> i32 ![] { return 1; }
impl agent Worker {
    flow factory<effect E>() -> (() -> unit ![E]) ![] {
        return () => { return; };
    }
}

flow outer<effect E>(worker: Worker) -> (() -> unit ![Console.stdout_write, E]) ![] {
    return worker.factory<![Console.stdout_write, E]>();
}

flow main() -> bool ![Error<CompileError>] {
    let callback = outer<![]>(compile(^Worker));
    callback();
    return true;
}
