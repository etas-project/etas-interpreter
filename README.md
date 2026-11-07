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
etas run program.es --cpu-threads 4
```

CPU workers are opt-in (`--cpu-threads N|auto`, default `1`, CLI cap `64`).
Checked pure named task functions with frozen data arguments/results run on a
fixed OS worker pool, including generic functions with concrete data types and
dependency flows with checked package ownership. Generic type and effect-row
bindings travel as data and are reconstructed inside the worker. Admission
checks the instantiated input/output freeze contract.
Closures, global captures, live handles, effects and
Handlers use the cooperative coordinator. Language `Concurrency(n)` still limits
each scope and its descendants independently of the host worker count.
Cancellation and execution budgets belong to the original run.
Step and wall-time exhaustion use the invocation entry span, so the task that
first observes a shared limit cannot change the reported diagnostic location.
Memory task scopes use coordinator-owned resource permits: distinct point keys
can overlap Host requests, equal keys wait, and multi-resource tasks acquire all
permits before starting. Permits remain held through Host waits and are released
on task completion or cancellation. Checked plans support string/integer/boolean keys passed
as arguments or captured by closures, including named helper calls. Scans acquire
the whole Store. Selectors or higher-order callbacks that cannot be resolved use
serial invocation admission; parent scope waits still allow nested tasks to run.
Resource wait/acquire/release events contain IDs and permit counts, without keys.
Workers use the parent's storage and syntax limits and preserve structured
language diagnostics. A worker panic settles its jobs as an infrastructure
failure. Small or unknown work stays on the coordinator; embedders can tune
`RunOptions::cpu_min_work` (default 10,000 estimated HIR operations). Zero is a
profiling override that dispatches every eligible task.

The manual release benchmark warms each case once, takes three samples, and
reports median wall time and per-task freeze/thaw, queue, bootstrap and execution
costs. It covers 1/2/4/8 workers, 1/4/32/1000 tasks, micro/medium/heavy tasks, and
map/join/nested scopes. It forces eligible tasks onto workers to measure the
dispatch crossover; it does not assert wall-clock speedups in CI.

```bash
cargo test --release cpu_dispatch_benchmark -- --ignored --nocapture --test-threads=1
```

Durable CPU scheduling events use logical worker IDs and dispatch/completion
sequences. OS thread IDs and durations are available through
`RunEventObserver::observe_cpu`. Report v3 and checkpoint v75 reject older
schemas; replay never depends on process-local thread IDs.

`crates/etas_interpreter/examples/concurrency_speedup.es` measures pure CPU work;
`handler_cpu.es` demonstrates Handler/`park` scheduling of the same worker path.
The older Console Handler example demonstrates serialized effect safety.
Checkpoints during active task scopes pause task admission and drain admitted
CPU jobs, Host requests and native kernels before capture. The snapshot preserves
scopes, ready queues, resource permits and waiters, parked continuations, task
machines, shared frames and consumed budgets. Resume continues those machines
without repeating completed Host requests or charging completed CPU work again.
Pending external requests must finish before the checkpoint can be created.

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
