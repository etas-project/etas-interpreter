# Etas Interpreter Repository Boundary

Status: `Draft`

Owner: `Architect`

Last updated: `2026-06-07`

## Responsibility

`etas-interpreter` owns the Phase 1 checked-HIR lightweight runtime.

It owns:

- public interpreter facade APIs consumed by the user-facing `etas` CLI;
- execution planning over `CheckedProject`;
- interpreter value model, frames, local slots, captures, control signals, and
  continuations;
- direct checked-HIR expression, statement, block, flow, agent, and intrinsic
  execution;
- interpreter-side pure builtin adaptation to `etas_builtin`;
- HIR-to-host request lowering and `InterpValue <-> HostValue` codecs;
- Phase 1 host-readiness checks for supplied handlers, authority grants,
  sandbox policy, budgets, and capabilities;
- checked-HIR retry, checkpoint, resume, workflow ledger, effect handler, and
  trace-ledger semantics;
- deterministic interpreter tests and fixtures.

Phase 1 interpreter design is recorded in
`docs/architect/phase1-interpreter-design.md`.

## Shared But Not Owned

The interpreter uses `etas-core` shared crates:

- `etas_core` for ids, diagnostics, spans, source maps, and shared utilities;
- `etas_builtin` for pure builtin kernels;
- `etas_host` for engine-neutral host values, model/tool/memory protocols,
  sandbox/workspace primitives, request ids, authority context, trace context,
  budget values, and reusable provider/tool/memory adapters.

The interpreter does not own provider-specific protocol clients when they are
engine-neutral. For example, OpenAI-compatible and Anthropic-compatible model
clients and typed persistent-memory backend adapters belong in `etas_host`.
The interpreter owns only checked-HIR lowering into `ModelRequest`,
`ToolRequest`, or `MemoryRequest`, plus conversion from host responses back to
`InterpValue`.

## Forbidden

This repository must not own:

- parser semantics;
- AST construction;
- HIR construction;
- name resolution;
- type checking;
- effect checking;
- standard-library declaration registry semantics;
- AIR construction, AIR verification, AIR optimization, or AIR execution;
- FIR analysis or optimization;
- production AIR scheduler, distributed workflow runtime, or AIR replay
  engine;
- provider protocol implementations that are reusable through `etas_host`;
- CLI rendering;
- LSP/editor protocol handling.

The interpreter may perform defensive runtime validation against checked facts,
but it must not infer new language semantics that belong to the frontend.

## Dependency Rule

Allowed dependencies:

```text
etas-interpreter -> etas-core
etas-interpreter -> etas-frontend
etas-interpreter -> etas_builtin
etas-interpreter -> etas_host
```

Forbidden dependencies:

```text
etas-interpreter -> etas
etas-interpreter -> etas-optimizing
etas-interpreter -> etas-runtime
etas-interpreter -> etas-ide
```

The interpreter executes checked source-shaped HIR only. External behavior must
go through supplied host services and shared `etas_host` protocol values. AIR
behavior belongs to the future runtime, not to this repository.
