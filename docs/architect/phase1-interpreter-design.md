# Phase 1 Interpreter Design

Status: `Draft`

Owner: `Architect`

Last updated: `2026-07-02`

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
share protocols and vocabulary;
do not share execution state or execution IR.
```

`etas-core` may define engine-neutral host values, request ids, model/tool/
memory protocols, sandbox/workspace primitives, retry/checkpoint vocabulary,
and trace ids. `etas-interpreter` owns `InterpValue`, frames, continuations,
handler stacks, HIR evaluation, and HIR-to-host lowering. The future AIR
runtime owns its own `AirValue`, scheduler, AIR instruction dispatch, and
AIR-to-host lowering.

## 2. Repository And Crate Shape

Phase 1 should keep `etas-interpreter` maintainable by starting with one main
crate and layered modules instead of many tiny crates.

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
          checkpoint.rs
          resume.rs
          workflow.rs
          trace.rs
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
pub struct Interpreter;

impl Interpreter {
    pub fn plan(
        &self,
        project: &CheckedProject,
        options: PlanOptions,
    ) -> PlanResult<InterpreterPlan>;

    pub async fn run_checked(
        &self,
        project: &CheckedProject,
        entry: EntryPoint,
        args: Vec<InterpValue>,
        host: &dyn HostServices,
        options: RunOptions,
    ) -> RunResult;
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
- return structured results and diagnostics without CLI rendering.

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

Source evaluation must run on one heap-backed `EvalMachine` for the complete
execution lifetime. Rust calls may implement one bounded machine transition,
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

There is exactly one active `EvalMachine` per `run_checked` or resume operation.
Host boundaries yield the machine without discarding its stack, and the driver
resumes that same machine with a typed response. In particular:

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

## 5. Evaluation Context

Evaluation state belongs to the interpreter.

```rust
pub struct EvalContext<'a> {
    pub checked: &'a CheckedProject,
    pub plan: &'a InterpreterPlan,
    pub handlers: HandlerStack,
    pub retries: RetryStack,
    pub host: &'a dyn HostServices,
    pub trace: TraceSink,
    pub fuel: Fuel,
}
```

`EvalContext` owns project-derived and run-wide services. `EvalMachine` owns
control state. The driver owns asynchronous host dispatch but not source-level
control flow or model/tool orchestration state.

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
}
```

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

The exact Rust names may evolve, but the boundary must stay explicit.

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
  diagnostics.

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
and workflow orchestration, but it should not create an AIR-like scheduler.

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
- do not duplicate non-idempotent host side effects unless a completed boundary
  ledger proves the result can be reused or the handler explicitly supports
  deduplication.

## 15. Diagnostics

Interpreter diagnostics should use shared diagnostic structures from
`etas_core` and source/HIR origins from `CheckedProject`.

Terminal execution failures have one diagnostic owner. Evaluator leaves and
budget checks return a typed fault or abort signal; they do not both append a
diagnostic and ask the driver to append another one. The public run boundary
materializes exactly one primary diagnostic for one terminal fault, with notes
or causes attached to that diagnostic when needed.

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

Live model tests are optional and must use local opt-in host adapters from
`etas_host`; they must not be required for ordinary offline test runs.
