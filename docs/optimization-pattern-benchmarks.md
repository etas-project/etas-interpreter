# HIR agent optimization benchmark

This benchmark covers the twelve optimization prototypes identified in the agent-system survey. Each transform is implemented in the frontend typed-HIR pipeline. Every performance transform has ten Etas programs compiled twice, once with `optimize_hir = false` and once with it enabled, then executed by the real interpreter.

The tests assert equal returned values, fewer deterministic interpreter steps, and fewer measured allocations. Wall-clock time is printed for observation but is not asserted because it is scheduler-sensitive. `AgentEffectLegality` is the P0 analysis gate from the survey: it has ten compile programs and deliberately reports no direct runtime benefit.

Run the reproducible performance set with:

```sh
cargo test -p etas_interpreter reduces_real --offline -- --nocapture
```

The following results are from 2026-11-05. Ranges cover all ten programs in each row.

| Pass | Programs | Interpreter steps, baseline → optimized | Allocations, baseline → optimized | Latest elapsed samples |
|---|---:|---:|---:|---|
| `PrepareVersionedRequest` | 10 | 255–477 → 173–272 | 200–354 → 134–192 | lower in 10/10 |
| `VersionedPreparationCSE` | 10 | 105 → 74 | 92 → 65 | lower in 10/10 |
| `SuffixDemandHistory` | 10 | 96–159 → 31 | 86–125 → 31 | lower in 10/10 |
| `FuseProjectionScans` | 10 | 265–397 → 220–330 | 204–284 → 157–222 | lower in 10/10 |
| `SynthesizeScopedIndex` | 10 | 147–213 → 31 | 118–160 → 28 | lower in 10/10 |
| `FuseSerializationBoundary` | 10 | 69 → 31 | 64 → 32 | lower in 10/10 |
| `LowerOwnedSnapshotCOW` | 10 | 40 → 21 | 41 → 24 | lower in 10/10 |
| `BatchStorageSemijoin` | 10 | 427 → 360 | 309 → 247 | lower in 10/10 |
| `ScheduleBoundedPipeline` | 10 | 767 → 700 | 565 → 503 | lower in 10/10 |
| `LowerResourceRegion` | 10 | 463 → 299 | 345 → 217 | lower in 10/10 |
| `QualityAwareContextPolicy` | 10 | 64 → 11 | 58 → 17 | lower in 10/10 |

`SuffixDemandHistory` reduces a complete literal history traversal to the demanded last entry. `QualityAwareContextPolicy` removes a pure context candidate only when typed-HIR use analysis proves that it never reaches a consumer. These exact rewrites reduce history/context materialization work; they do not claim a reduction in live model-input tokens because the observable returned model input is intentionally unchanged. A live-model quality/token comparison belongs to an explicitly policy-changing P3 mode rather than these semantics-preserving defaults.

The frontend suite contains the source-level pass assertions, including the ten `AgentEffectLegality` programs. The interpreter suite owns runtime equivalence and cost measurements. No model API is required for this exact benchmark.
