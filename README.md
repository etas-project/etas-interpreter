# Etas Interpreter

The Phase 1 lightweight runtime for Etas.

`etas-interpreter` plans and executes a checked project directly from typed HIR.
It can perform real model, tool, memory, console, filesystem, command, network,
TLS, stream, secret, approval, browser, and session operations when the caller
provides matching host services and authority. It is not an AIR executor; the
AIR-backed production runtime belongs to Phase 2.

## Execution Contract

The interpreter accepts `etas_frontend::CheckedProject`. Parsing, project
discovery, package resolution, type/effect checking, and static handler
validation have already happened before planning begins.

The execution path is:

```text
CheckedProject
  -> select entry and build ExecutionPlan
  -> validate Phase 1 support and host readiness
  -> initialize EvalMachine, ledgers, trace, and checkpoint state
  -> evaluate checked HIR
  -> dispatch pure intrinsics through etas_builtin
  -> dispatch observable operations through etas_host::HostServices
  -> apply handlers/continuations and produce an execution report
```

Source effects describe possible behavior; they do not grant host authority.
Missing adapters, denied actions, unavailable providers, memory conflicts, and
exhausted budgets fail closed with source-aware execution diagnostics. The
interpreter must not return an empty value, no-op success, mock network result,
or package-private host fallback.

## What It Executes

- flow calls, blocks, statements, expressions, returns, loops, patterns, and
  local mutation;
- records, variants, collections, indexing/slicing, methods, nominal values,
  and checked conversions;
- handler literals/applications, `finish`, `abort`, retry, and continuation
  control supported by checked HIR;
- pure standard intrinsics through the shared builtin registry;
- agents and model inference through an explicit model host service;
- tools and observable substrate operations through typed host requests;
- typed persistent memory, sessions, checkpoints, resume, replay identity,
  trace events, budgets, and effect/action ledgers.

Requested actions and escaping effects remain separate. For example,
`Agent.ask(...)` records `Agentic.infer[A]` as an internal requested action and
requires model support, but does not expose that action as the agent's public
effect. Default-handled standard actions likewise remain visible in action and
trace facts without escaping in the public effect row.

## Heap-Backed Calls

Source calls run on the heap-backed `EvalMachine` frame stack rather than by
recursing through the native Rust stack. The default call-depth budget is
4,096 and the configurable hard cap is 65,536. Exhaustion produces one Etas
diagnostic. Raising or removing the cap requires evaluator-wide frame and
continuation semantics; a local recursive shortcut is not acceptable.

## Crate and Layers

The repository currently contains one crate: `etas_interpreter`.

| Layer | Responsibility |
|---|---|
| `api` | Async/blocking public facade and caller-facing reports |
| `plan` | Entry selection, checked-fact validation, support classification, and execution plan construction |
| `eval` | Expression, statement, block, handler, method, and boundary evaluation |
| `control` | Heap frames, continuations, pending boundaries, and control signals |
| `value` | Interpreter values, aggregates, resources, and host-value codecs |
| `intrinsic` | Pure intrinsic classification, adaptation, and dispatch |
| `host` | Host request preparation, availability, and readiness diagnostics |
| `orchestration` | Ledgers, retries, checkpoints, sessions, and resume state |
| `driver` | Main execution loop and host-boundary continuation |
| `diagnostics` | Source-aware execution failures |

The primary entry points are:

- `Interpreter::plan` to validate and prepare checked HIR;
- `Interpreter::run_checked` for asynchronous execution with caller-provided
  `HostServices`;
- the blocking facade under `etas_interpreter::api`;
- `validate_host_readiness` for explicit preflight validation.

## Repository Boundary

This repository owns direct checked-HIR execution. It does not:

- parse `.es` files or repeat frontend inference;
- read manifests, lockfiles, or package stores;
- configure CLI flags or render user-facing command output;
- lower HIR to AIR or execute AIR;
- own FIR optimization or a distributed production scheduler;
- define private host bindings for EDK packages.

Shared host protocols and adapters live in
[`etas-core`](https://github.com/etas-project/etas-core). The user-facing CLI
selects runtime profiles and constructs the supplied services.

## Use Through the CLI

The normal entry point is the [`etas`](https://github.com/etas-project/etas)
CLI:

```bash
etas check --phase1 path/to/program.es
etas run path/to/program.es
etas run path/to/project --profile local
```

Trace and checkpoint workflows:

```bash
etas run program.es --trace-out /tmp/etas-trace.json
etas replay /tmp/etas-trace.json

etas run program.es --checkpoint-dir /tmp/etas-checkpoints
etas resume 0 --checkpoint-dir /tmp/etas-checkpoints
```

Runtime profiles should be preferred over a large set of legacy host
environment variables. Secret values remain references to environment or
secret-provider entries and must not be written into committed manifests,
traces, or checkpoints.

## Build and Verify

The workspace requires Rust `1.85` or newer and the compatible `etas-core` and
`etas-frontend` revisions.

```bash
cargo build -p etas_interpreter
```

Standard verification:

```bash
cargo fmt --all -- --check
cargo test -p etas_interpreter --offline
cargo clippy -p etas_interpreter --offline -- -D warnings
```

Interpreter changes should also run the relevant CLI and fixture tests in the
top-level `etas` repository. Host-backed success tests must use explicit local
adapters or loopback services; a mock is valid only in a test explicitly
testing mock behavior.

## Architecture Documents

- [Phase 1 interpreter architecture](docs/architect/phase1-interpreter-design.md)
- [Repository boundary](docs/architect/repository-boundary.md)

## License

Etas Interpreter is distributed under the terms of both the
[MIT License](LICENSE-MIT) and the
[Apache License (Version 2.0)](LICENSE-APACHE). You may choose either license.
