# Etas Interpreter

Executes Etas programs directly from checked HIR.

Checked HIR is the frontend's resolved, type- and effect-checked program.
This interpreter is the execution engine behind `etas run`.

It evaluates source computations, applies effect handlers, and dispatches
external operations through explicit host services. Model calls, networking,
and other host-backed operations require matching adapters and permissions;
successful static checking does not itself grant access.

Experimental. To run a program, use the
[Etas CLI](https://github.com/etas-project/etas#quickstart).

## Bootstrap

Requires Rust 1.85+ and a native C/C++ build toolchain.

```sh
git clone https://github.com/etas-project/etas-interpreter.git
cd etas-interpreter
cargo build -p etas_interpreter
```

This builds the interpreter library, not a separate command-line executable.
Use the Etas CLI to run source files, or run the library tests directly:

```sh
cargo test -p etas_interpreter
```

## Documentation

- [Interpreter design](docs/architect/phase1-interpreter-design.md)
- [Repository boundaries](docs/architect/repository-boundary.md)

[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE)
