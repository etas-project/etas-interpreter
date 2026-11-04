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

`SuffixDemandHistory` reduces a complete literal history traversal to the demanded last entry. The exact mode of `QualityAwareContextPolicy` removes a pure context candidate only when typed-HIR use analysis proves that it never reaches a consumer.

The separate `quality_context_token_budget` compile option enables the policy-changing P3 mode. Its ten Etas programs return 18–20 whitespace-delimited context units without the policy and exactly 8 recent units with it. The interpreter test asserts the reduced input and suffix retention; it does not pretend that the two policy outputs are semantically identical.

## Live-model token and quality evaluation

`tools/quality_context_live_eval.py` reads the same ten Etas fixtures and sends baseline/optimized pairs to the official China-region MiniMax endpoint. The committed report in `docs/quality-context-live-eval.json` was generated with `MiniMax-M3.1-Flash` on 2026-11-05.

| Metric | Baseline | P3 budget | Change |
|---|---:|---:|---:|
| API `prompt_tokens`, 10 calls | 2,422 | 2,314 | -108 (-4.46%) |
| Expected fact retained | 10/10 | 10/10 | unchanged |
| Aggregate network/model latency | 14,221.692 ms | 14,319.819 ms | +98.127 ms |

Latency is recorded but not claimed as a benefit for this run because request scheduling and generation variance dominate these short prompts. The measured P3 benefit is lower provider-counted input tokens with unchanged fact retention on the ten workload probes.

Run it again with:

```sh
python3 tools/quality_context_live_eval.py --api-key-file /path/to/minimax-key
```

The frontend suite contains the source-level pass assertions, including the ten `AgentEffectLegality` programs. The interpreter suite owns runtime equivalence and cost measurements. No model API is required for this exact benchmark.
