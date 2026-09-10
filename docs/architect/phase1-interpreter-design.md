# Phase 1 Interpreter Design

Status: `Draft`

Owner: `Architect`

Last updated: `2026-09-10`

## 1. Purpose

`etas-interpreter` is the Phase 1 checked-HIR lightweight runtime. It executes
the checked project contract produced by `etas-frontend` directly over HIR.

It is intentionally more capable than a toy evaluator:

- it executes pure local Etas computation;
- it dispatches shared pure builtins through `etas_builtin`;
- it executes model, tool, network, filesystem, command, approval, and typed
  persistent-memory API boundaries through supplied host services;
- it owns Phase 1 retry, checkpoint, resume, workflow orchestration, effect
  handler, and trace-ledger semantics over checked HIR.

It does not execute AIR. It must not construct a pseudo-AIR, FIR, CFG, DFG, or
generic execution graph to simulate the future runtime. Phase 2 `etas-runtime`
owns AIR instruction dispatch, optimized scheduling, production replay, durable
distributed workflow execution, and AIR-level recovery.

The design rule is:

```text
share host protocols and engine-neutral execution lifecycle mechanisms;
keep language evaluation state and execution IR engine-owned.
```

`etas-core` may define engine-neutral host values, request ids, model/tool/
memory protocols, sandbox/workspace primitives, retry/checkpoint vocabulary,
trace ids, and live scope/cancellation/operation ownership in `etas_host`.
`etas-interpreter` owns `InterpValue`, frames, continuations,
handler stacks, HIR evaluation, and HIR-to-host lowering. The future AIR
runtime owns its own `AirValue`, scheduler, AIR instruction dispatch, and
AIR-to-host lowering.

The shared lifecycle contract is [Shared Execution Lifecycle](../../../etas-core/docs/architect/etas-execution-design.md).
It defines infrastructure state, not shared HIR/AIR evaluation state. The
integration below implements that contract over the checked-HIR machine.
The invocation/lifecycle design below is the accepted architecture target;
implementation acceptance requires the tests in Section 17. It is not a claim
that the current code already implements every listed interface.

## 2. Repository And Crate Shape

Phase 1 should keep `etas-interpreter` maintainable by starting with one main
crate and layered modules instead of many tiny crates.
This is a target layout, not an inventory of existing files. `api/` is the
existing public Rust interface, not an HTTP API or a separate server.
`api/lifecycle/control.rs` already exists; `invocation.rs` and the scheduler
organization below are target additions. File responsibilities, rather than
renaming alone, define this migration.

```text
etas-interpreter/
  crates/
    etas_interpreter/
      src/
        lib.rs

        api/
          mod.rs
          options.rs
          result.rs
          blocking.rs
          entry_args.rs
          codec/
          lifecycle/
            mod.rs
            invocation.rs
            control.rs

        plan/
          mod.rs
          pipeline.rs
          entry.rs
          slots.rs
          globals.rs
          resources.rs
          dispatch.rs
          readiness.rs

        value/
          mod.rs
          primitive.rs
          aggregate.rs
          support.rs
          resource.rs
          codec.rs

        eval/
          mod.rs
          expr.rs
          stmt.rs
          block.rs
          flow.rs
          agent.rs
          call.rs
          pattern.rs
          assign.rs
          safe_point.rs
          machine/
            mod.rs
            state.rs
            frame.rs
            step.rs
            budget.rs
            resume.rs
            snapshot/
              mod.rs
              continuation.rs
              call_target.rs
              frame.rs
              model.rs

        driver/
          mod.rs
          drive.rs
          lifecycle/
            mod.rs
            run.rs
            shutdown.rs
          scheduler/
            mod.rs
            ready.rs
            group.rs
          dispatch/
            mod.rs
            host_dispatch.rs

        control/
          mod.rs
          signal.rs
          handler.rs
          continuation.rs
          retry.rs
          loop_limit.rs
          fuel.rs

        host/
          mod.rs
          services.rs
          model.rs
          tool.rs
          memory.rs
          console.rs
          filesystem.rs
          command.rs
          network.rs
          approval.rs

        intrinsic/
          mod.rs
          pure.rs
          host.rs
          dispatch.rs

        orchestration/
          mod.rs
          checkpoint/
            mod.rs
            snapshot.rs
            restore.rs
            validate.rs
          retry/
            mod.rs
            attempt.rs
            replay.rs
          trace/
            mod.rs
            events.rs
            correlation.rs
          ledger.rs

        diagnostics/
          mod.rs

        testing/
          mod.rs
```

The split is functional, not crate-heavy. Additional crates such as
`etas_interpreter_value` or `etas_interpreter_test_support` may be extracted
later only after the API boundary is stable and the extraction removes real
complexity.

The public facade is `etas_interpreter`. User-facing `etas` CLI code should
depend only on this facade, not on evaluator internals.

## 3. Public API

Recommended facade shape:

```rust
use std::future::Future;

pub struct Interpreter;

impl Interpreter {
    pub fn plan(
        &self,
        project: &CheckedProject,
        options: PlanOptions,
    ) -> PlanResult;

    pub fn create_run<'a>(
        &self,
        project: &'a CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> RunInvocation<'a>;

    pub fn create_resume<'a>(
        &self,
        project: &'a CheckedProject,
        checkpoint: &'a InterpreterCheckpoint,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> RunInvocation<'a>;

    pub fn run_checked<'a>(
        &self,
        project: &'a CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &'a dyn HostServices,
        options: RunOptions,
    ) -> impl Future<Output = RunResult> + 'a;
}
```

Responsibilities:

- accept a checked frontend project, not raw syntax;
- build an execution plan from HIR ids, type facts, effect facts, std intrinsic
  ids, and source maps;
- consume frontend-resolved spec facts, generic instantiation facts, and
  effect-row substitutions;
- validate interpreter readiness for the selected entry;
- validate the user entry ABI, including `main(args: Array[string]) -> i32`;
- validate supplied host services and authority grants;
- execute the entry flow;
- return explicit execution outcome, operation evidence, cleanup report and
  diagnostics without CLI rendering.

The interpreter must not parse source, lower HIR, resolve names, infer types,
solve spec constraints, instantiate effect-row polymorphism, or infer effects.
It may repeat defensive checks against checked facts, but it must not invent
semantic facts that belong to the frontend.

Spec polymorphism and effect-row polymorphism are compile-time facilities. By
the time execution reaches the interpreter:

- calls to `std.stream.read_until_limit<S ~ ByteStream>` already carry a concrete
  checked argument type such as `TlsStream` plus a satisfied `ByteStream` fact;
- behavioral spec method calls already resolve to a concrete impl method or a
  checked dispatch fact;
- row-polymorphic flow calls such as `twice<T, effect E>` already carry
  type/effect-row substitutions in `TypeFacts` and instantiated effect summaries
  in `EffectFacts`.

The interpreter uses those facts to select std intrinsic descriptors, host
requests, handlers, and dispatch entries. It must not fall back to string-name
matching such as "if the value looks like a stream, allow it".

### 3.1 Controlled Invocation API

`RunInvocation<'a>` owns one invocation and its single-use execution right. It
borrows the checked project and Host services, and owns entry arguments/options
or the resume input reference. It is neither cloneable nor a second evaluator.
Construction establishes ownership but does not plan, evaluate source, dispatch
Host work, or spawn a background task. `execute(self)` drives planning/readiness
or restore validation and then the same checked-HIR execution machinery.

```rust
let run = interpreter.create_run(project, entry, args, host, options);
let control = run.control();
// The caller arranges a signal/UI task that can use control while this awaits.
let result = run.execute().await;
```

Allocate a fresh invocation scope for every run and resume. If attached to a
parent execution scope, use the shared child-admission contract rather than
reusing the parent's scope as the invocation. `RunOptions` carries execution
configuration, not a reusable mutable root or an independently supplied control
handle. Replace the raw invocation-scope injection in `RunOptions::execution`;
the returned control must always address the scope owned by this invocation.

| Public operation | Contract |
|---|---|
| `RunInvocation::control()` | Clone a control/observation handle for this invocation only |
| `RunInvocation::execute(self)` | Consume execution ownership; return one final `RunResult` after local termination |
| `RunControl::stop(reason)` | Idempotently request stop; neither block for cleanup nor announce completion |
| `RunControl::status()` | Read a shared lifecycle snapshot; it is observational, not an admission permit |
| `RunControl::join()` | Observe the immutable shared `TerminationReport`; do not start or repeat evaluation |
| `RunControl::wait_stopped(deadline)` | Bound observation and return shared `StopWait`; timeout retains ownership and control |

`RunControl` contains only engine-neutral, thread-safe control/observation state,
not `InterpValue`, HIR frames, or the result value. Do not expose a mutable root
`ExecutionScope` through it. All observers may await termination; only the
execution future produces the language value. Preserve structured Host
lifecycle errors if observation/control infrastructure fails.

The embedding caller must drive the execution future and keep the Host
supervisor alive. A control handle does not poll either. Dropping a control or
a join waiter does not stop the invocation. Dropping the invocation or its
execution future requests stop and relinquishes the engine body, including
when dropped before the first poll. Create the ownership guard in construction,
not only inside the first poll of an async function. Host supervisors retain
registered in-flight work and outcome evidence independently of borrowed HIR
state; dropping an evaluator must not detach or forget such work.
Stopping an unpolled invocation records the request; its owner must still be
driven or dropped to relinquish the body. A pre-stopped invocation skips source
execution when driven. Waiting alone cannot release its execution ownership.

Keep `run_checked`, `resume_checkpoint`, and blocking entry points as convenience
facades over this same owner and driver. Their ownership guard must also exist
before first poll; the future-returning facade above preserves `.await` usage
while permitting eager ownership construction. No second
planning/evaluation path, hidden `'static` spawn, whole-project clone, or private
blocking timeout evaluator is allowed.

### 3.2 Terminal Results And Arbitration

Final result shape:

```rust
pub enum RunOutcome {
    Completed(InterpValue),
    Failed(RunFailure),
    Cancelled(CancellationCause),
}

pub struct RunResult {
    pub outcome: RunOutcome,
    pub termination: TerminationReport,
    pub diagnostics: Vec<Diagnostic>,
    pub events: Vec<WorkflowEvent>,
    pub checkpoints: Vec<InterpreterCheckpoint>,
}
```

`RunFailure` distinguishes language failure, preparation/restore rejection and
execution faults, with typed causes and real source origins. Do not encode these
as a provider-error string. Internal evaluation/preparation outcomes are not
public `RunResult` objects with `termination: None`. Even a readiness failure
settles its invocation, without dispatching Host work, before producing a final
result. Remove the old independent `value: Option<InterpValue>` field so a
cancelled/failed result cannot simultaneously claim a completed value.

Use the one shared scope lifecycle: `Running -> Draining -> Terminated`,
`Running -> Stopping -> Terminated`, and `Draining -> Stopping`. Body completion
enters drain, not immediate termination. Stopping prohibits new business work;
only cleanup of already-owned resources is admitted. Interpreter control state
must not become a duplicate of this shared state machine.

The engine selects the language outcome; core records cancellation and local
completion evidence. A language failure that initiates sibling cancellation
remains the primary failure, not a generic cancelled result. Concurrent causes
are retained, and completion/stop arbitration uses the shared synchronized
commit contract, not `select!` polling order. Once terminal publication wins,
later stop cannot rewrite it. Body success during draining is not yet terminal.

`TerminationReport` separates operation evidence and local cleanup from the
language result: confirmed work, partial progress, or an unknown remote outcome
must survive cancellation. Cleanup failures remain secondary evidence and must
not overwrite the original language failure. A successful language value alone
does not prove successful cleanup. Unknown remote completion can coexist with
settled local resources.

A shutdown wait timeout is `StopWait::TimedOut(pending)`, never a final
`RunResult`. The owner remains live and observable, including for repeated
waits. Normal final results require local termination; they must not contain
pending local work disguised as completed cleanup.

### 3.3 Lifecycle File Responsibilities

| Location | Responsibility |
|---|---|
| `lib.rs` | Narrow facade construction/delegation, not orchestration loops |
| `api/lifecycle/invocation.rs` | Borrowed input lifetime, single-use owner/guard, public execute/control methods |
| `api/lifecycle/control.rs` | Restricted public stop/status/wait interface over shared lifecycle primitives |
| `api/result.rs` | Final outcome/failure/report contract; codec mirrors it without guessing missing fields |
| `driver/lifecycle/run.rs` | Drive fresh/restore inputs through the common preparation and evaluator path |
| `driver/lifecycle/shutdown.rs` | Coordinate engine body settlement, cleanup observation and final result assembly |
| `driver/scheduler/` | Checked-HIR branch readiness and SPEC combinator decisions, not Host lifetime machinery |

There is no separate `api/lifecycle/completion.rs` state machine or copied
termination-report type. Reuse `etas_host::execution`. Signal handling and
process-exit policy stay in the CLI/application.

## 4. Planning Pipeline

Planning is a pass pipeline. Evaluation is not.

```text
CheckedProject
  -> PlanPipeline
      ValidateCheckedProjectPass
      BuildEntryPlanPass
      BuildSlotLayoutPass
      BuildGlobalTablePass
      BuildResourceHandleTablePass
      BuildIntrinsicDispatchTablePass
      ComputeReachableActionMediationPass
      ValidateRuntimeMediationPass
  -> InterpreterPlan
  -> EvalContext
  -> execute entry flow
  -> RunResult
```

`PlanPipeline` may use `etas_utils::pipeline` because it is an artifact
construction process. The evaluator loop must remain owned by
`etas-interpreter`; it is not a pass manager.

Recommended plan data:

```rust
pub struct InterpreterPlan {
    pub entry: EntryPlan,
    pub slots: SlotLayoutTable,
    pub globals: GlobalTable,
    pub resources: ResourceTable,
    pub dispatch: IntrinsicDispatchTable,
    pub action_mediation: ActionMediationTable,
    pub diagnostics: Vec<Diagnostic>,
}
```

Planning should fail before execution when:

- the selected entry cannot be found;
- required HIR/type/effect facts are missing;
- unresolved symbols remain in reachable code;
- the program asks for AIR-only behavior;
- required host services, active handler frames, action grants, or residual
  checks are absent for reachable host boundaries;
- configured fuel, loop, retry, or checkpoint limits are invalid.

Missing host services or denied authority are execution-readiness or execution
configuration diagnostics, not static frontend correctness diagnostics.

### 4.1 Call-Stack Safety Boundary

Each sequential execution branch runs on one heap-backed `EvalMachine` for its
complete lifetime. Rust calls may implement one bounded machine transition,
but Etas flow, lambda, spec-method, agent, and source-tool calls must never form
a recursively nested Rust evaluator call chain.

The machine owns an explicit stack:

```rust
pub struct EvalMachine {
    stack: Vec<EvalFrame>,
    active_call_depth: u32,
    signal: Option<ControlSignal>,
}

pub enum EvalFrame {
    Block(BlockFrame),
    Expr(ExprFrame),
    Call(CallFrame),
    Continuation(ContinuationFrame),
    Handler(HandlerFrame),
    Retry(RetryFrame),
    ModelLoop(ModelLoopFrame),
    SourceToolReturn(SourceToolReturnFrame),
}
```

`ExecutionLimits.max_call_depth` limits active Etas call frames, not native
Rust stack frames:

```text
default max_call_depth = 4096
configurable hard cap  = 65536
```

Call-depth accounting must be O(1). Every machine push, pop, unwind, retry
truncate, and snapshot restore goes through stack-management methods that keep
`active_call_depth` synchronized. A pass that scans the full stack before every
call is prohibited because it turns depth-N recursion into O(N^2) execution.

There is one machine for the entry branch of `run_checked` or resume. Only
explicit structured concurrent branches introduce separately owned machines;
ordinary calls, model rounds and handlers do not. Host boundaries yield a
machine without discarding its stack, and the driver resumes that same machine
with a typed response. In particular:

```text
Etas call
  -> EvalMachine transition loop
  -> model request yields to driver
  -> model response resumes ModelLoopFrame
  -> source tool pushes an Etas CallFrame on the same machine
  -> tool result resumes SourceToolReturnFrame
  -> model loop continues
```

The driver must not call `drive_eval_signal(..., EvalMachine::new())` from an
already active execution. Model rounds, repair attempts, tool-call ids, queued
tool calls, boundary ledger keys, and the outer continuation are machine state;
otherwise call limits reset and a checkpoint inside a source tool cannot resume
the enclosing agent computation.

A pending-tail-call global, a special case for `return f(...)`, or a nested
machine used only for source tools is not an acceptable substitute for this
evaluator-wide model.

### 4.2 Cooperative Scheduling And Cancellation

`eval/safe_point.rs` observes the current scope's cancellation signal along with
step and time budgets. `ExecutionSafePointScheduler` also owns quantum accounting
and decides whether to continue, yield, cancel, or report the prescribed limit
failure. Machine/driver/intrinsic code consumes that decision instead of keeping
independent polling intervals, cancellation flags and quantum counters.
`eval/machine/state.rs` distinguishes completed value,
pending Host boundary, execution fault, cancellation, and `YieldToScheduler`.
Cancellation is a control outcome, not a fabricated provider or handler error.

`eval/machine/step.rs` runs a bounded quantum of transitions. On quantum expiry,
retain the current signal and stack and return `YieldToScheduler`.
`driver/drive.rs` gives the async executor a scheduling opportunity and resumes
the same machine without supplying a fake `Unit` or resetting step/call limits.
The single-thread executor must be able to run the task that requests stop or
advances a deadline while Etas evaluates a CPU-only loop.

Use O(1) safe-point checks; do not scan scopes, frames or active requests on
every expression. Long-running builtin kernels need a verified per-operation
work bound or a chunked computation interface driven by the evaluator. Check
between chunks; checking every fixed number of machine transitions is not a
bound on a single unbounded transition. Pure kernels do not depend on Host
scopes or acquire host effects. The quantum is an implementation parameter,
not a new source limit or a guaranteed wall-clock bound for uninterruptible
host code. Share monotonic-clock infrastructure with budget checks so quantum
and deadline tests do not rely on sleeps or machine speed.

Execution scopes follow structured branch/deadline ownership. Ordinary flow
calls and handler applications inherit the current scope. Dynamic handler
frames remain engine-owned; scope cancellation propagates into handler-produced
actions without inventing a second handler selection algorithm.

### 4.3 Structured Branch Scheduling

`driver/scheduler/ready.rs` owns the ready-branch queue and completion wakeups;
`group.rs` owns branch membership, result slots and the selected std combinator's
completion policy. Reuse Tokio wakeups and asynchronous waiting primitives; do
not build another thread pool or spawn a Rust task per expression/action.
Scheduling HIR machines is Interpreter work, not a shared Host operation.

Each active branch has its own machine, local control state and child scope.
Checked project/plan/facts are shared read-only. Budgets are the parent's shared
ledger with any stricter child limits, not cloned allowances. Branch-local value
state preserves Etas value semantics, while inherited handler environments do
not share mutable continuations or one-shot resume ownership across branches.
Do not hold a whole-`EvalContext` mutex while polling another branch or awaiting
Host work. Bound active branches and pending work according to the checked
combinator/limit contract; yield fairly between runnable branches and back to
the embedding executor.

The [Concurrency SPEC](../../../etas/docs/design/16-concurrency.md) defines the
group behavior, independently of the cancellation-token implementation:

| Construct | Group decision |
|---|---|
| `join` / `try_join` | On an unhandled branch error, preserve the triggering error, cancel unfinished siblings and await their local settlement; honor an explicitly result-collecting std variant |
| `collect` | Collect branch results; an ordinary branch error does not stop its siblings |
| `race` | Select the first successful branch, not the first completed error; cancel unfinished losers and await settlement before returning |
| `map_concurrent` | Enforce `Concurrency(n)` and the declared fail-fast/collecting variant; keep stable result association |

An all-failed race follows its declared std variant; the engine must not invent
an empty successful value. External parent cancellation applies to every group
variant and is not converted to collected ordinary errors. A branch failure
that is already captured by `?` is a returned value, not an unhandled failure.
Dynamic handler inheritance and checked effect/resource/trace-order constraints
remain in force. Cancellation of a losing branch neither rolls back its
confirmed effects nor erases attempted actions from trace.

## 5. Evaluation Context

Evaluation state belongs to the interpreter.

```rust
pub struct EvalContext<'a> {
    pub checked: &'a CheckedProject,
    pub plan: &'a InterpreterPlan,
    pub host: &'a dyn HostServices,
    pub execution: ExecutionScope,
    pub budget: ExecutionBudget,
    pub trace: TraceSink,
    pub safe_points: ExecutionSafePointScheduler,
}
```

This excerpt describes a branch's access to immutable project data and explicitly
shared run services. `EvalMachine` owns its frames, local slots, handler/retry
control state and resumable model/tool state; do not introduce a second stack in
`EvalContext`. Host services, trace correlation and budget accounting are shared
through their defined interfaces, not by sharing a mutable evaluator between
branches. The driver schedules machines and asynchronous Host dispatch; it does
not duplicate source-level evaluation or handler/model-loop transitions.

Frame layout must be derived during planning from checked HIR bindings. Runtime
lookup should use HIR ids, local slots, and plan tables, not source strings.
This prevents the interpreter from silently redoing or diverging from name
resolution.

## 6. Value Model

`InterpValue` is the interpreter's internal language value. It is not
`etas_host::HostValue` and not a frontend type.

```rust
pub enum InterpValue {
    Unit,
    Bool(bool),
    Int(IntValue),
    Float(FloatValue),
    Char(char),
    String(String),
    Bytes(Vec<u8>),

    Array(ArrayValue),
    List(Vec<InterpValue>),
    Map(Vec<(InterpValue, InterpValue)>),
    Set(Vec<InterpValue>),
    Range(RangeValue),
    Slice(SliceValue),
    Record(RecordValue),
    Variant(VariantValue),

    Prompt(PromptValue),
    PromptPart(PromptPartValue),
    Message(MessageValue),
    Schema(SchemaDescriptor),

    Resource(ResourceHandle),
    Flow(FlowValue),
}
```

Implementation enum variants may use Rust casing. Source-facing diagnostics and
dumps must render PL type names exactly, for example `bool`, `i32`, `f64`,
`string`, `bytes`, and `unit`; there are no source-level `Bool`, `Int`,
`Float`, or `Unit` type names.

The interpreter value model must preserve the PL collection distinction:

- source `[a, b]` evaluates to `InterpValue::Array`;
- source `[a; b]` and `a :: xs` evaluate to `InterpValue::List`;
- source `{ k => v }` evaluates to `InterpValue::Map`;
- source `#{v}` evaluates to `InterpValue::Set`;
- source `[start, end)` and `(start, end]` evaluate to `InterpValue::Range`;
- slicing an array, slice, bytes, or range evaluates to `InterpValue::Slice`,
  `bytes`, or `Range` according to checked type facts.

Implementations may use copy-on-write vectors, persistent list nodes, or view
objects internally, but observable behavior must remain value-semantic. The
interpreter must not silently coerce `Array` and `List`; conversions require an
explicit checked std function or language operation.

`HostValue` conversion is engine-owned:

```text
InterpValue <-> HostValue
AirValue    <-> HostValue
```

The shared `HostValueCodec` trait belongs in `etas_host`; the interpreter
implements the codec for `InterpValue`. Host adapters must not see frames,
continuations, HIR ids, or interpreter-only support values.

## 7. Top-Level `let` And Resources

Top-level `let` has two interpreter-visible classifications from frontend
facts:

- deterministic/effect-free constant;
- compiler-known resource handle.

The interpreter should not execute arbitrary top-level initializers. Planning
builds:

```rust
pub struct GlobalTable {
    pub constants: Vec<GlobalConst>,
}

pub struct ResourceTable {
    pub handles: Vec<ResourceHandleDescriptor>,
}
```

For typed persistent memory:

```etas
alias ProjectMemorySchema = MemoryRegion[{ Papers: Store[PaperId, PaperRecord] }];

let ProjectMemory =
    std.memory.region[ProjectMemorySchema](
        stable_id = "project_memory",
        store = "project-main"
    );
```

`ProjectMemory` becomes a declarative `ResourceHandle`. Constructing the handle
does not connect to or mutate the backend at module load. Actual reads/writes
are ordinary checked calls/member operations that lower to interpreter memory
host requests with region-sensitive action footprints such as
`Memory.read[ProjectMemory.Papers]` or
`Memory.write[ProjectMemory.Papers]`.

Nominal type identity is enforced before interpretation. The interpreter must
not recover from a type mismatch by structurally comparing a nominal type with
its representation. For `type UserId = string`, raw `string` values are not
accepted where `UserId` is expected unless the checked program contains an
explicit constructor/conversion/accessor that the type checker accepted. The
runtime representation may be compact or erased internally, but execution,
host-value codecs, checkpoints, and diagnostics must follow checked nominal
type facts rather than guessing from the underlying value shape.

Bodyless nominal types such as external handles are only constructible through
trusted std/package/runtime APIs that provide checked facts. The interpreter
must fail closed if it sees a bodyless nominal value without a verified
producer.

If an effectful or non-stable top-level initializer reaches the interpreter,
the interpreter reports a defensive diagnostic and refuses to run.

## 8. Evaluation Semantics

The evaluator executes checked HIR in source shape.

Expression evaluation includes:

- literals;
- local slots and captured values;
- records, fields, tuples, arrays, lists, maps, sets, ranges, slices, enums,
  options, and results;
- checked indexing and slicing using frontend `TypeFacts`, not ad hoc runtime
  shape guessing;
- unary and binary operators;
- `if` and `match` expressions;
- flow calls;
- agent calls such as `Agent.ask(input)`;
- pipeline stage application after frontend lowering/resolution;
- standard intrinsic calls by checked intrinsic id.

Statement execution includes:

- block-scoped immutable `let`;
- block-scoped mutable `var`;
- assignment to valid mutable targets;
- expression statements with value discard;
- `return`;
- `break` and `continue`;
- `for` and `while` with required limits;
- `retry`;
- `handle` / `perform` / `resume`;
- postfix `?` as an `Error[E]` capture boundary;
- standard checkpoint calls such as `runtime.checkpoint(...)`.

Statement-position `if` and `match` discard their value. Only
`HirBlock.final_expr` contributes to a block result.

`for` iteration must consume checked iterable facts or checked expression types.
At minimum it must support `Array[T]`, `List[T]`, `Set[T]`, `Range[I]`,
`Slice[T]`, and any string/bytes iteration semantics explicitly accepted by the
PL SPEC. A loop over an unsupported value is an interpreter bug if type checking
accepted it; execution should report a defensive diagnostic rather than falling
back to unit.

## 9. Control Signals

Control flow must not be represented as ordinary values.

```rust
pub enum ControlSignal {
    Value(InterpValue),
    Return(InterpValue),
    Break,
    Continue,
    Perform(PerformedAction),
    Resume(InterpValue),
    Abort(InterpreterDiagnostic),
    Cancelled(CancellationCause),
}
```

`Cancelled` represents external execution cancellation, distinct from
source-level `abort`/`finish` and typed service errors. It propagates through
ordinary error handlers and `?` without being captured and never triggers retry.
A local service cancellation may still produce the checked `StreamError.Cancelled`
when the surrounding execution scope is healthy. Budget failure keeps its SPEC
error semantics; the exhausted scope cancels unfinished work before reporting
that error to an eligible enclosing handler. A stopped scope cannot resume
ordinary computation by catching a cleanup or I/O error.
Source `finish`/`abort` retain their checked handler/control meaning; neither is
a general-purpose shutdown finalizer. Runtime cleanup releases owned resources
without evaluating arbitrary business handler arms after cancellation.

Effect handler execution:

- `perform` produces `ControlSignal::Perform`;
- `handler { ... }` evaluates to a handler value without executing arm bodies;
- `handle body with a handler argument` evaluates that argument, pushes the checked
  handler frame for the dynamic body scope, then evaluates `body`;
- the nearest compatible handler arm receives the action payload;
- resumable actions create a one-shot continuation;
- `resume` consumes the continuation exactly once;
- actions returning `never` cannot resume;
- handler-produced effects and result compatibility must be read from frontend
  `HandlerValueFact` / `HandleApplicationFact`, not recomputed from source
  strings;
- frontend already checks these rules, but the interpreter must enforce them
  defensively.

The interpreter should not implement multi-shot continuation semantics unless
the PL SPEC explicitly adds them later.

Reusable handler values are ordinary interpreter values with restricted
contents:

```rust
pub struct HandlerValue {
    pub fact: HandlerValueId,
    pub captured_env: CaptureSet,
}
```

They may be stored in immutable top-level `let`, passed as parameters, returned
from flows, or selected by conditionals. Creating a handler value grants no
authority and performs no effect. Authority is checked only when a performed
action or host/tool boundary is reached. A handler can recover, return a
fallback, request approval, or resume a checked continuation, but it cannot add
capabilities, widen the active effect boundary, bypass sandbox, or modify
policy/limit state.

Postfix `?` execution must use checked type/effect facts:

- evaluate the operand expression under a local synthetic capture boundary for
  the checked `Error[E]`; if the operand is a block expression, the boundary
  covers the whole block execution;
- if the operand completes with value `v`, return `Ok(v)`;
- if the operand performs `Error[E].raise(err)` for the checked captured error,
  consume that action and return `Err(err)`;
- if checked conversions were required by effect checking, apply only those
  recorded conversions;
- propagate all non-captured actions and control signals normally;
- report an interpreter diagnostic if a `HirExpr::Try` lacks a checked
  `TryCaptureFact`.

The interpreter must not treat `r: Result[T, E]` followed by `r?` as unwrapping
or early return. That invalid source shape should be rejected by frontend
type/effect checking; defensive runtime handling should fail closed if it is
encountered.

`?` is not visible to statement execution as a terminator. Statement evaluation
observes only the AST/HIR shape produced by the frontend: `let` and `return`
consume their own semicolon syntax, while a final block expression can be a
`HirExpr::Try` value.

## 10. Host Boundary

The interpreter never directly opens sockets, spawns processes, reads/writes the
filesystem, calls models, invokes tools, or mutates persistent memory. It lowers
checked HIR host boundaries to `HostServices`.

Recommended boundary:

```rust
pub trait HostServices {
    async fn model(&self, req: ModelRequest) -> HostResult<ModelResponse>;
    async fn tool(&self, req: ToolRequest) -> HostResult<ToolResponse>;
    async fn memory(&self, req: MemoryRequest) -> HostResult<MemoryResponse>;
    async fn console(&self, req: ConsoleRequest) -> HostResult<ConsoleResponse>;
    async fn filesystem(&self, req: FsRequest) -> HostResult<FsResponse>;
    async fn command(&self, req: CommandRequest) -> HostResult<CommandResponse>;
    async fn network(&self, req: NetworkRequest) -> HostResult<NetworkResponse>;
    async fn approval(&self, req: ApprovalRequest) -> HostResult<ApprovalDecision>;
}
```

The exact Rust names may evolve, but the boundary must stay explicit. Each
request also carries the shared live `OperationContext`; it is not transmitted
as provider payload or reconstructed from static effect rows.

`etas_host` owns reusable provider/tool/memory/sandbox adapters:

```text
ModelRequest  -> OpenAI/Anthropic/local provider -> ModelResponse
ToolRequest   -> MCP/HTTP/process protocol       -> ToolResponse
MemoryRequest -> SQLite/Postgres/vector backend  -> MemoryResponse
ConsoleRequest -> stdin/stdout/stderr adapter     -> ConsoleResponse
WorkspacePath/SandboxPolicy safety primitives
```

`etas-interpreter` owns:

```text
checked HIR call/effect/action fact -> HostRequest
InterpValue <-> HostValue
authority/readiness checks for this execution
mapping HostError -> interpreter diagnostic
```

Action dispatch registers dynamic occurrences under the execution scope before
mediation; registration does not bypass checked authority. Pure handlers may
produce no Host request, while one action may produce several correlated Host
operations. Reuse the existing boundary occurrence ledger and request IDs,
preserving parent/attempt relationships. Two equal action names are not the same
operation, and retry is not another execution of the same occurrence identity.

`driver/dispatch/host_dispatch.rs` is the common Host lifecycle integration
point. Service-specific dispatch retains typed request/result conversion.
There must be no raw service path that omits registration, scope cancellation,
completion evidence or trace accounting. Do not blindly race every Host future
against a token and drop the loser: the adapter must provide its documented
cancellation-safe or managed-completion contract.

Record a confirmed external result before abandoning a continuation due to
cancellation. Preserve partial or unknown results even when a normal value
cannot be delivered. Do not use `escaping_effects` to identify active work:
handler elimination and `?` leave real requested actions and their operations
subject to scope ownership, cancellation, authority and trace.

`driver/lifecycle/shutdown.rs` settles the engine body and coordinates the
shared cleanup mechanism. Concrete interruption, transaction observation,
process reap and resource release stay in Host adapters/supervisors. Cleanup
has its own monotonic bound, protected from the already-triggered business
signal, but admits only release of owned resources with existing authority.
It must not replenish business budgets or launch new model/tool work. If the
bound expires, retain pending ownership; do not pretend an uninterruptible
thread or remote request was killed. The embedding caller must keep the Host
supervisor alive while pending operations are being observed.

Deny-by-default behavior is required. If a reachable host boundary lacks a
checked action fact, active explicit handler, standard host service, action
grant, active trace-spec allowance, sandbox approval, or budget, the interpreter
reports a structured diagnostic instead of attempting execution. Escaped package
actions are never resolved through package metadata fallback; the caller or
application must provide the handler.

`std.io` is a standard-library surface and a runtime-recognized intrinsic
family, but it is still a host boundary. The checked-HIR interpreter must lower
`std.io.read_all`, `std.io.read_line`, `std.io.print`, `std.io.println`, and
`std.io.eprintln` to `HostServices::console` through checked action facts. These
operations publicly escape only `Error[IOError]`, while their requested actions
are `Console.stdin_read_all`, `Console.stdin_read_line`,
`Console.stdout_write`, or `Console.stderr_write`. Host failures raise the
checked typed error, and postfix `?` may capture that error into
`Result[_, IOError]`. Capturing the error does not erase the requested console
action from trace-spec, grant, or trace checks. The evaluator must not write
directly to the process terminal. Tests should supply fake console buffers
through `HostServices`.

## 11. Intrinsic Dispatch

Dispatch is by checked symbol or `StdIntrinsicId`, not by source string.

```text
Pure intrinsic
  -> etas_builtin

Host intrinsic
  -> HostServices

Interpreter control intrinsic
  -> checkpoint / trace / approval / limit handling

Unsupported intrinsic
  -> diagnostic
```

`etas_builtin` in `etas-core` owns shared pure kernels. The interpreter owns
only adaptation:

- `InterpValue` to `BuiltinValue`;
- call `call_pure_intrinsic`;
- `BuiltinValue` to `InterpValue`;
- `BuiltinError` to interpreter diagnostic.

Pure builtin behavior must not be reimplemented locally.

Runtime-recognized host intrinsics must remain explicit:

```text
std.io.*              -> Console.* action -> HostServices::console
std.memory.*          -> Memory.read/write[R] action -> HostServices::memory
std.runtime.approval.* -> Approval.request action -> HostServices::approval
model/agent inference -> Model.use[M] / Inference -> HostServices::model
tool calls            -> ToolCall boundary -> HostServices::tool
```

The word "runtime" in an intrinsic descriptor is a lowering/dispatch
classification. It is not permission to bypass `etas_host` or collapse all
host authority into a single opaque runtime call.

## 12. Agent Execution

An `agent` declaration is one model-inference boundary. Its body is the
prompt/context harness and must have checked result type `Prompt`.

Calling an agent:

```text
Writer.ask(input)
input ~> Writer
```

executes:

```text
evaluate agent body/context harness -> Prompt
Prompt + model config + tools + authority -> ModelRequest
HostServices::model(request) -> ModelResponse
decode/validate response -> declared output type
```

Multi-step orchestration, approval gates, persistent memory writes, and
post-output validation remain ordinary `flow` code around the agent call.

## 13. Typed Persistent Memory

Persistent memory is a typed host boundary, not a language declaration item.

Interpreter responsibilities:

- read resource handle descriptors from checked top-level `let` facts;
- lower typed store operations to memory host requests;
- preserve region/store identity in trace and checkpoint records;
- include memory versions in checkpoint records when the host returns them;
- report missing backend, denied authority, or version conflicts as structured
  diagnostics;
- preserve confirmed commit/version, partial results and unknown commit
  evidence even when the enclosing run is cancelled.

Host responsibilities:

- bind resource handles to concrete backends;
- execute read/write/select/update operations;
- return version metadata;
- enforce backend-specific safety.

The interpreter must not treat `Map[K, V]` as a persistent store. `Map` is an
ordinary in-memory value; `Store[K, V]` inside `MemoryRegion[S]` is persistent
memory support.

## 14. Retry, Checkpoint, Resume, And Workflow Ledger

Phase 1 needs real checked-HIR execution support for retry, checkpoint, resume,
and workflow orchestration. Its branch scheduler drives HIR machines; it must
not construct AIR instructions or a second workflow execution IR.

Within `orchestration/`, `checkpoint/` owns typed snapshot construction, restore
validation and restoration; `retry/` owns attempt/replay decisions; `trace/`
owns event/correlation models; `ledger.rs` owns occurrence completion evidence.
These modules consume shared Host operation evidence, not a second cancellation
registry. Artifact encoding remains in `api/codec/`, and active model/retry
control stays in the machine frames. Split the existing flat checkpoint and
event models by these responsibilities, not into renamed duplicate pathways.

Use an execution ledger instead of a second IR:

```rust
pub enum WorkflowEvent {
    StepStarted(WorkflowStepId),
    StepCompleted(WorkflowStepId),
    HostRequestSent(HostRequestId),
    HostResponseReceived(HostRequestId),
    CheckpointCreated(CheckpointId),
    RetryAttemptStarted(RetryAttemptId),
    RetryAttemptFinished(RetryAttemptId),
}
```

Checkpoints are interpreter-owned records:

```rust
pub struct InterpreterCheckpoint {
    pub entry: EntryPoint,
    pub args: Vec<InterpValue>,
    pub machine: MachineSnapshot,
    pub handlers: HandlerSnapshot,
    pub retry_state: RetrySnapshot,
    pub trace: TraceSnapshot,
    pub resource_versions: ResourceVersionSnapshot,
    pub completed_host_boundaries: HostBoundaryLedger,
}
```

`MachineSnapshot` is a typed execution snapshot, not pre-encoded JSON:

```rust
pub struct MachineSnapshot {
    pub frames: Vec<MachineFrameSnapshot>,
}

pub enum MachineFrameSnapshot {
    Block(BlockFrameSnapshot),
    Expr(ExprFrameSnapshot),
    Call(CallFrameSnapshot),
    Continuation(ContinuationSnapshot),
    Handler(HandlerFrameSnapshot),
    Retry(RetryFrameSnapshot),
    ModelLoop(ModelLoopFrameSnapshot),
    SourceToolReturn(SourceToolReturnFrameSnapshot),
}
```

The snapshot layer may use stable ids, values, and dedicated DTO enums. It must
not store `serde_json::Value` inside interpreter or orchestration structures.
JSON is only an artifact codec at the API boundary. Restoring a snapshot must
validate all HIR ids, frame kinds, call targets, handler/retry nesting, and
recompute `active_call_depth` from typed call frames.

A checkpoint created inside a source tool must include the enclosing model
loop, source-tool return point, outer Etas calls, handlers, retries, completed
host boundaries, and model repair state. Resume must produce the same final
value and observable boundary sequence as uninterrupted execution.

This is sufficient for Phase 1 debugging, retry, resume, and deterministic
tests. It is not the future AIR runtime's production checkpoint format.

Retry behavior:

- retry only where the source has explicit retry semantics and valid limits;
- record attempt numbers and boundary events;
- do not retry missing action grants, denied trace-spec/admission checks, or
  sandbox violations;
- never automatically retry external run cancellation;
- retry a write with unknown completion only when a verified
  reconciliation/idempotency contract resolves the risk;
- do not duplicate non-idempotent host side effects unless a completed boundary
  ledger proves the result can be reused or the handler explicitly supports
  deduplication.

Backoff waits observe the current cancellation signal. Each permitted retry
retains the parent budget and authority, checks admission again before dispatch,
and records a distinct attempt. Do not map cancellation to a generic retryable
Host error or restart a cancelled scope with a fresh token inside the retry
loop.

### 14.1 Cancellation And Durable Boundaries

Scope IDs and cancellation reasons may appear in trace correlation, but active
tokens, registrations, supervisor handles, sockets and cleanup guards are not
checkpoint state. Resume creates a new invocation scope with fresh cancellation
state while preserving durable occurrence identities and consumed budgets.
Capture budget state as an immutable snapshot at checkpoint creation, not a
clone of the live budget ledger. Later consumption must not mutate the saved
checkpoint. Resume preserves the trace-parent relationship without restoring
the old invocation's live control context.

The completed-boundary ledger stores confirmed outcomes, not a guessed result
for a cancelled wait. If a checkpoint must describe pending/uncertain work,
encode it as separate typed evidence and validate it during restore. A boundary
whose completion cannot be safely represented or reconciled prevents automatic
resume with a precise diagnostic. Never silently omit it or replay an unknown
write. Evolve the checkpoint schema explicitly when adding durable fields and
reject incompatible artifacts without defaulting missing execution evidence.

Run cancellation does not automatically create a resumable checkpoint. The
application chooses recovery policy; the interpreter guarantees that a reported
checkpoint and completion ledger truthfully describe supported recovery.

### 14.2 Storage Intents And Receipts

Consume the accepted [public storage contract](../../../etas-core/docs/architect/etas-storage-design.md#8-standard-library-and-engine-integration).
The Session API quartet is now specified; the general Memory intent API still
needs its complete source declarations synchronized separately. StdRegistry owns
the declarations; the interpreter's intrinsic/value/codec layers adapt them to the existing
`MemoryClient`/`SessionClient`, without implementing SQL or a second receipt store.

- Preparation creates an immutable typed intent and runtime-issued operation
  reference before dispatch. It does not mutate the backend and is not evaluated
  as a deterministic pure builtin. Identity allocation failure is explicit.
- Checkpoint codecs preserve target, typed payload, condition and operation
  identity, with existing byte/depth/node bounds and secret handling. They do not
  serialize live connections or grants. Restore validates identity and current
  authority without generating a replacement reference.
- Commit checks the approved write action and invokes the managed Host boundary.
  Convert every confirmed/unknown outcome into its declared nominal/ADT shape;
  never map unknown completion to `unit`, a generic retryable I/O error or a fresh
  write attempt. Update ledger evidence even if the run is concurrently cancelled.
- Reconcile checks read authority and only queries evidence. Unresolved/expired
  results remain distinct from confirmed non-commit. A matching recorded outcome
  is reused rather than re-evaluating the original condition as a new mutation.
- `?` handles Error effects, not `WriteOutcome.Unknown`. Convenience-write errors
  must retain operation references when their API cannot return an outcome.
  Cancellation remains run control, independent of commit certainty.

Persisting an intent/reference before dispatch is an explicit caller/checkpoint
decision. An in-memory intent alone is not a cross-crash recovery guarantee.
Session context publication reuses these lifecycle rules: `history_page` returns
history/fence/context; `prepare_context` binds content and fence to an operation
reference; `publish_context` verifies that binding and publishes conditionally;
`reconcile_context` only queries evidence. Preserve the prepared content/fence
and reference across supported recovery. Reconciliation does not resume
cancelled execution or grant authority to a restored session.

Source-level acceptance must cover prepare-without-write, intent persistence and
restore, CAS conflicts, lost acknowledgements, same-identity replay after a
successful write changes the version, receipt expiry and cancellation during
commit. Tests use a real persistent adapter and independently observe the stored
result; a stubbed response codec test is insufficient.

### 14.3 Application-Owned Context Processing

The interpreter executes summary/tokenization flows as ordinary checked source
code. EDK/applications select providers, models, tokenizer implementations,
prompts, output validation and retry policy. Existing safe points, budgets,
Host authority and trace apply to these flows without a special compactor path.

Host Session supplies bounded history and conditional publication of already
produced content. The publication fence detects concurrent history/context
changes and returns a conflict; the interpreter does not silently reread,
re-summarize, merge or delete messages. A selection helper may use existing
published context but must not trigger an implicit model call. Per the SPEC,
`SummaryPlusRecent` without a summary selects recent turns and exposes absence;
it does not fabricate an empty summary or claim to return full history.
Exhausted `ContextTokens` follows ordinary limit failure semantics, not an
automatic call to reduce the context size.

Keep source-history/producer provenance and trust intact through publication,
codec roundtrips, context selection and prompt construction. A publication
receipt proves the backend outcome/version/durability, not summary accuracy or
permission to inject it into a trusted instruction channel. Failure/cancellation
while generating content leaves published context unchanged; uncertain
publication still needs reconciliation.

Remove reliance on Host `SessionCompactor` model/tokenizer callbacks and implicit
`SummarizeWhen` behavior now that the SPEC removes `SessionConfig.compaction`.
Do not replace them with interpreter-local providers, Debug concatenation or a
test summarizer.
A production summarizer configuration is not an interpreter completion gate.
Do not remove configured retention, archival, deletion or physical storage
compaction; these remain separate runtime/storage operations with bounded work
and explicit effects on replay availability.

## 15. Diagnostics

Interpreter diagnostics should use shared diagnostic structures from
`etas_core` and source/HIR origins from `CheckedProject`.

Terminal execution failures have one diagnostic owner. Evaluator leaves and
budget checks return a typed fault or abort signal; they do not both append a
diagnostic and ask the driver to append another one. The public run boundary
materializes exactly one primary diagnostic for one terminal fault, with notes
or causes attached to that diagnostic when needed.

Cancellation and cleanup reports have the same single diagnostic owner. The
driver must not turn scope cancellation into missing-handler, empty-value, or
generic retryable Host diagnostics. Pending cleanup is reported separately from
the run's language error and never converted to successful termination.

Recommended codes:

```text
interpreter::MissingEntry
interpreter::InvalidArguments
interpreter::MissingCheckedFact
interpreter::UnsupportedPhase2RuntimeFeature
interpreter::MissingHostHandler
interpreter::MissingActionGrant
interpreter::DeniedHostAuthority
interpreter::TraceSpecDenied
interpreter::SandboxViolation
interpreter::CheckpointStoreUnavailable
interpreter::InvalidResumeState
interpreter::RetryLimitExceeded
interpreter::FuelExhausted
interpreter::InvalidIntrinsicDispatch
interpreter::HostError
```

The interpreter should return diagnostics instead of panicking for
user-visible failures. Internal invariants may use debug assertions, but
release execution should prefer structured failure.

## 16. Dependency Direction

Allowed:

```text
etas-interpreter -> etas-core
etas-interpreter -> etas-frontend
etas-interpreter -> etas_builtin   # crate provided by etas-core
etas-interpreter -> etas_host      # crate provided by etas-core
```

Forbidden:

```text
etas-interpreter -> etas
etas-interpreter -> etas-optimizing
etas-interpreter -> etas-runtime
etas-interpreter -> etas-ide
```

`etas-interpreter` must not depend on CLI rendering, IDE/LSP code, AIR runtime
code, or optimizing middle-end code.

## 17. Test Direction

Interpreter tests should cover:

- plan pipeline construction;
- entry selection and argument validation;
- slot layout and source/HIR id stability;
- expression evaluation;
- statement-position value discard;
- flow calls and recursion with fuel;
- 5,000 and 20,000 level non-tail recursion without native stack overflow;
- approximately linear recursion scaling, with call-depth checks remaining
  O(1) per machine transition;
- one diagnostic, rather than duplicate leaf/driver diagnostics, when call
  depth or execution fuel is exhausted;
- local `let`, local `var`, assignment, and block result rules;
- top-level constant and resource-handle planning;
- record/list/map/set/enum/option/result values;
- pattern matching;
- pure intrinsic dispatch through `etas_builtin`;
- host intrinsic dispatch through fake `HostServices`;
- missing handler and denied authority diagnostics;
- sandbox violation diagnostics;
- model/tool/network/filesystem/command/memory dispatch with deterministic
  fake handlers;
- agent call request construction and response decoding;
- effect handler and one-shot `resume`;
- retry attempt accounting;
- checkpoint creation and resume from interpreter checkpoint records;
- checkpoint/resume across nested calls, handlers, retries, model rounds, and
  source-tool calls using one machine stack;
- cumulative call-depth enforcement across agent/model/source-tool boundaries;
- fail-closed decoding of malformed typed machine-frame artifacts;
- typed persistent-memory version metadata in checkpoints;
- workflow ledger determinism;
- unsupported AIR/runtime feature diagnostics;
- deterministic `run_checked` output.

### 17.1 Lifecycle Acceptance

Tests exercise `create_run`/`create_resume` and the convenience facades, not just
a cancellation flag. Use barriers/channels, injected monotonic clocks and
watchdogs; retain real adapter coverage alongside deterministic unit tests.

| Scenario | Required evidence |
|---|---|
| Stop before first poll followed by drive, or owner drop before first poll | No source/Host execution; body ownership is released and termination becomes observable; waiting alone does not drive an unpolled owner |
| Waiter/control drop and repeated/late waits | Run is unaffected by observer drop; no lost wakeup, rerun or duplicate terminal publication |
| Execution-future drop with pending I/O | Host supervisor still owns the operation; evidence survives loss of borrowed evaluator state |
| CPU loop/deep recursion/large builtin on a current-thread executor | Another task can request stop and make progress; frames/limits survive yielding; chunked work is actually bounded |
| Body success followed by drain | Parent cannot report termination while a child or owned cleanup is still active |
| Register/complete/stop races | Each request is rejected or registered; confirmed/partial/unknown evidence is not overwritten by generic cancellation |
| Repeated action names and sibling cancellation | Distinct occurrence identities; stopping one child does not directly stop a healthy sibling/parent |
| `join`/`collect`/`race`/bounded map | Correct SPEC result policy; race ignores early errors until success/all-failed; no return with unsettled children |
| Nested handlers, `?`, budget failure and retry | External stop is not captured/retried; ordinary typed errors retain their semantics; child limits never refresh the parent budget |
| Cleanup timeout | `StopWait::TimedOut` retains observable pending work, not a false final `RunResult`; locks are not held across waits |
| Checkpoint/resume | Fresh live scope, immutable consumed-budget evidence, no duplicate confirmed writes and explicit rejection of unreconciled unknown writes |
| Public report/codec | Outcome and value cannot contradict; final report is required; cancellation and cleanup do not create duplicate primary diagnostics |

Real loopback network, command supervision, pending input and SQLite completion
tests must demonstrate interruption or documented non-interruption, not simply
return a mocked cancelled error. See the shared
[acceptance matrix](../../../etas-core/docs/architect/etas-execution-design.md#8-implementation-and-acceptance).

Live model tests are optional and must use local opt-in host adapters from
`etas_host`; they must not be required for ordinary offline test runs.

### 17.2 Migration Boundary

Implement ownership and result invariants first, then unify scheduler/safe-point
decisions and Host settlement, then migrate run/resume/CLI and recovery tests.
This is an implementation order, not reduced feature scope. Remove the raw
root-scope injection/control escape hatch, independent optional result value,
intermediate `RunResult` construction, duplicated evaluator polling policies,
and any private evaluator or unowned Host dispatch retained by old call sites.
Preserve mandatory admission/cancellation checks at irreversible Host dispatch
points. Preserve supported public convenience facades by delegating to the one
owner/driver.
Renaming an old evaluator or moving it behind `RunInvocation` is not acceptance.
