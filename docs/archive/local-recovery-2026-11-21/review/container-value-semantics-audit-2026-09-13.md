<!-- Historical recovery snapshot, archived 2026-11-21. Not a current specification, implementation status, or engineering task assignment. The current language SPEC and main implementation take precedence. -->

# Container And Value-Semantics Audit

Date: 2026-09-13. Scope: current local implementation, not an optimization proposal approved for implementation.

## Scope And Counting

This audit covers 12 local collection families (Array, List, Slice, Map, Set, Deque, Queue, Stack, PriorityQueue, OrderedMap, OrderedSet, Range), plus string, bytes, Record, Tuple, Option, Result, user enum, Json and Prompt: **21 value families**. Nominal and Trust wrappers are cross-cutting identity/validation constraints, not additional collection algorithms.

The paths inspected include construction, lookup, update, concatenation, slicing, equality, iteration, parameter/local reads, closure capture, pure intrinsic conversion, prompt/host conversion and checkpoint capture. Database, network and session-storage algorithms are outside this inventory.

There are **31 optimization groups**, not 31 independent bugs or a count of every clone call. Several groups share a root cause. There are also **three collection correctness groups** and an independently reproduced frontend/runtime iteration gap. Some output allocation is required by the API and is explicitly distinguished below.

Notation:
- n/m: input element counts; k: selected/output element count; f: record/variant field count.
- S: owned payload bytes/nodes copied. An outer vector clone costs O(n) plus cloning each contained value.
- Nested Rc-backed aggregates usually share their nested backing during that clone; String, Bytes, Tuple and boxed ADT payloads can copy recursively. It would be inaccurate to call every snapshot a full recursive deep copy.
- Complexity here follows code inspection. This audit did not measure allocation counts, wall-clock speedups or memory peaks.

Reference SPEC: [collection definitions](/Users/hrtan/Projects/aplas/etas/docs/design/02-general-programming-constructs.md:830), [value semantics](/Users/hrtan/Projects/aplas/etas/docs/design/02-general-programming-constructs.md:968). The SPEC explicitly permits sharing/COW and does not require eager deep copying at call boundaries.

## Correctness Before Optimization

### C1. PriorityQueue Is Currently A Stack

[local.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:299) appends (priority, value); pop removes the last entry. No priority selection occurs. Reversing insertion order for two distinct priorities changes the first returned value.

This is independent of whether the intended queue is minimum-first or maximum-first. Either policy must select the same priority when the same two entries are inserted in a different order. A heap or other ordered representation must use the language's checked ordering semantics, not debug strings or insertion order.

### C2. Set Literals Do Not Enforce Uniqueness

[aggregate_expr.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/aggregate_expr.rs:424) wraps the evaluated vector directly as a Set. SetValue is a vector wrapper without deduplication at construction.

The source probe below iterates twice and returns 2, although the SPEC says a Set has unique values:

```etas
let s = #{1, 1};
var total = 0;
for x in s limit Iterations(4) { total = total + x; }
return total;
```

Enforce uniqueness at every legitimate creation/restoration boundary, with checked equality/hash semantics and stable iteration. Fixing just the literal renderer is insufficient.

### C3. OrderedMap / OrderedSet Have No Ordered Representation Or Traversal

[OrderedMap insert](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:344) and [OrderedSet insert](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:375) append new entries. Inserting 2 then 1 produces backing order [2, 1]. The [interpreter iterator](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/loop_control.rs:213) walks that backing without sorting.

Unsorted backing alone would not be a language bug if the public iterator independently ordered values. Here neither layer provides that ordering. The source-level iteration path also has G1 below, so the probe separately inspected returned values; it did not successfully execute an OrderedMap source loop.

### G1. Frontend Iteration Coverage Does Not Match Runtime Branches

[solver::iterable_item](/Users/hrtan/Projects/aplas/etas-frontend/crates/etas_types/src/solver/mod.rs:833) accepts Array, List, Set, Slice, MemorySelection and Range. It does not accept the advanced collections that the interpreter iterator handles.

An OrderedMap loop is rejected with "for-loop iterator must be an iterable collection or range". This was reproduced independently. Other advanced collection omissions are visible statically in the same match but were not individually executed in this audit.

### G2. List API Coverage Needs A Separate Conformance Test

[std declarations](/Users/hrtan/Projects/aplas/etas-core/crates/etas_std/src/modules/core/collections.rs:177) register List.extend, but [local list dispatch](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:105) and [the expression-argument implementation](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/collections.rs:80) only implement len/is_empty/push/pop. The SPEC also describes head/tail.

These are declaration/implementation gaps to verify through checked-source API tests, not operations that should be silently counted as optimized or implemented. This audit did not execute a separate List.extend/head/tail probe.

## Optimization Inventory

### Collection Algorithms And Backing

| ID | Operation / current implementation | Cost and missing optimization |
| --- | --- | --- |
| O01 | Array.push/pop: [local.rs:79](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:79) | Snapshot the entire array even for an owned temporary. Repeated accumulation copies O(n^2) outer elements. Pop copies even when only removing the last entry. Need a consuming update path that can reuse unique backing, and a correct shared-value path. |
| O02 | List.push/pop and cons: [local.rs:117](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:117), [aggregate_expr.rs:70](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/aggregate_expr.rs:70) | Vec snapshot plus insert(0)/remove(0), O(n) per front operation. This misses structural sharing for the SPEC's functional list. Changing only COW does not eliminate front shifts. |
| O03 | Array.extend and Array/List +: [local.rs:99](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:99), [operator_expr.rs:335](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/operator_expr.rs:335) | Copies both inputs then builds a fresh vector, O(n+m) plus payload cloning. No unique-left capacity reuse or consumed-right transfer. Do not conflate this with the unimplemented List.extend API. |
| O04 | Deque.push_front/back/pop_front/back: [local.rs:197](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:197) | All updates snapshot; front updates additionally shift the vector. A deque-capable backing is needed in addition to value ownership handling. |
| O05 | Queue.push/pop: [local.rs:232](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:232) | Push snapshots; pop snapshots and remove(0). A drain of n entries copies/shifts O(n^2) elements. Use queue state with efficient front removal, preserving old values. |
| O06 | Stack.push/pop: [local.rs:259](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:259) | Vec is an appropriate basic layout, but unconditional snapshot defeats amortized push and constant-time pop on uniquely owned values. |
| O07 | Map.get/contains_key/index: [local.rs:174](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:174), [index_access.rs:129](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/index_access.rs:129) | Copies all entries before a linear search, even for a boolean contains result. First remove copy-on-read; then add a checked-key index while retaining stable iteration. |
| O08 | Map indexed assignment: [assign_nested.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/assign_nested.rs:16) | Linear key lookup plus root/path COW. Repeated key updates copy/scan the container. Requires indexed storage and the O24 place-ownership correction, not just a different map lookup helper. |
| O09 | OrderedMap contains/get/insert: [local.rs:319](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:319) | Snapshot plus linear search. Inserts also lack ordering (C3). Use an ordered data structure or indexed ordered backing; do not sort a copied map for every lookup. |
| O10 | OrderedSet contains/insert: [local.rs:360](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:360) | Snapshot plus linear membership check, including duplicate inserts that leave the logical set unchanged. Ordering is also missing (C3). |
| O11 | PriorityQueue push/pop: [local.rs:286](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:286) | Snapshot O(n) on each operation in addition to C1. Correct heap operations should be logarithmic on reusable backing; persistent/shared updates need their own cost contract. |
| O12 | Array/Slice/Bytes slicing: [slice_access.rs:87](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/slice_access.rs:87), [SliceValue](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/value/aggregate.rs:104) | Selected elements/bytes are copied with to_vec. SliceValue owns a separate vector instead of backing + offset + length. A small view should not require copying k elements. Byte input may already have been cloned by a local read. |
| O13 | Slice.to_array: [local.rs:164](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:164) | Always copies k elements. Materializing a distinct Array can legitimately cost O(k); reuse is an opportunity only when ownership/layout permit it. Do not classify every such copy as a defect. |
| O14 | Range iteration: [loop_control.rs:239](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/loop_control.rs:239) | Pre-builds up to the iteration limit as a Vec. It is bounded, but not lazy; a cursor can produce each next value with constant auxiliary state, including when the loop breaks immediately. |
| O15 | Range slicing: [slice_access.rs:136](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/slice_access.rs:136), [range_values](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/slice_access.rs:227) | Expands the entire original range before selecting the slice and reconstructing a Range. A tiny slice of a huge range costs O(n) time/space, not O(k) or O(1). Calculate checked endpoints directly; do not add an arbitrary small range limit as the fix. |

### Text And Structured Values

| ID | Operation / current implementation | Cost and missing optimization |
| --- | --- | --- |
| O16 | String/Bytes local and parameter reads: [Frame::get](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/control/frame.rs:81), [InterpValue](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/value/primitive.rs:10) | Owned String/Vec cloning copies all bytes when the caller only needs a read. No shared immutable byte/text backing or last-use transfer protocol. This affects len, predicates, encoding and argument passing as well as updates. |
| O17 | String index and len: [index_access.rs:170](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/index_access.rs:170), [builtin len](/Users/hrtan/Projects/aplas/etas-core/crates/etas_builtin/src/text/string.rs:8) | Index first counts all characters and then calls nth. Even index zero scans the whole string. Avoid the unconditional error-length scan on success; repeated scalar indexing may need a cursor/index. Unicode character indexing cannot simply become byte indexing. |
| O18 | String concatenation: [operator_expr.rs:330](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/operator_expr.rs:330) | push_str already reuses an owned left temporary. However, s = s + piece first clones the old frame value, so repeated accumulation still copies growing prefixes. Need ownership-aware evaluation or an explicit builder, not another formatting wrapper. |
| O19 | trim/lowercase/uppercase and text predicates: [text/string.rs](/Users/hrtan/Projects/aplas/etas-core/crates/etas_builtin/src/text/string.rs:3) | The kernels borrow inputs, but the call path copies source strings. trim allocates even for unchanged content; case conversion generally needs output allocation. Optimize input access and valid unchanged-result reuse, not Unicode semantics. |
| O20 | lines/split/join: [text/string.rs:33](/Users/hrtan/Projects/aplas/etas-core/crates/etas_builtin/src/text/string.rs:33) | lines/split materialize all strings; join receives converted/copied input strings and creates a temporary Vec of references. Output construction is required by these APIs. The current join itself is linear, not a repeated-concatenation quadratic algorithm. Shared text slices/borrowed ABI can reduce intermediate copies. |
| O21 | Record field read: [field_access.rs:162](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/field_access.rs:162) | Snapshot every field, then search by name. Reading one scalar can copy unrelated large payloads. Use a borrowed lookup and checked field-layout/slot information. |
| O22 | Named variant construction and record/variant patterns: [variant.rs:19](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/variant.rs:19), [pattern.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/pattern.rs:125) | Repeated field-name searches can be O(f^2), and named construction clones already evaluated payloads. Precompute checked field permutations/slots; consume owned payloads once while preserving source evaluation order. |
| O23 | Aggregate equality: [aggregate.rs:43](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/value/aggregate.rs:43) | Array/List/Slice/Map/Set/Record equality snapshots both operands before comparing. This allocates even for different lengths or a first-element mismatch. Borrowed short-circuit comparison avoids copies. Identity fast paths require proof that the value equality is reflexive for that type. |

### Cross-Cutting Evaluation And Boundary Costs

| ID | Operation / current implementation | Cost and missing optimization |
| --- | --- | --- |
| O24 | Nested local assignment: [assign.rs:31](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/assign.rs:31), [assign_nested.rs:16](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/assign_nested.rs:16) | Frame::get clones the root Rc while the slot still owns it; make_unique therefore sees sharing even without a user alias. It copies the root and updated path. Introduce an explicit owned-place/commit protocol, retaining the old value correctly on errors and suspension. |
| O25 | Container for-loop setup and per-element access: [loop_control.rs:199](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/loop_control.rs:199), [continue_for_loop](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/loop_control.rs:111) | Snapshot all input entries, then clone the selected element again each iteration. break or a small iteration limit does not prevent the first full collection copy. Use a snapshot-preserving backing + cursor; retain checkpoint/continuation semantics. |
| O26 | Tuple/Option/Result/enum reads, constructors and unwrap: [primitive.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/value/primitive.rs:10), [fast_path.rs:73](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/intrinsic/pure/fast_path.rs:73) | Owned vectors/boxes recursively clone payloads. Borrowed fast-path arguments clone payloads for Some/Ok/Err/unwrap even when the call owns them. Split non-consuming inspection from consuming extraction/construction without dropping nominal or checked ABI validation. |
| O27 | Pure builtin ABI: [call.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/intrinsic/pure/call.rs:11), [abi/input.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/intrinsic/pure/abi/input.rs:55) | Aggregate snapshots are followed by recursive conversion to a second value graph. Record conversion repeatedly searches and removes fields (O(f^2)). Map length lacks the aggregate count fast path. Introduce typed borrowed/owned projections and prevalidated field layouts; do not bypass the type-safe ABI. |
| O28 | Checkpoint capture: [snapshot/value.rs:53](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/machine/snapshot/value.rs:53) | Snapshot clones are followed by capture into another owned tree. Shared subvalues are captured repeatedly by this path. Borrow directly during capture; immutable backing/deduplication is a separate format/lifetime design. Sharing mutable live state with an old checkpoint is forbidden. |
| O29 | Json/Prompt/Host conversion and secret scanning: [prompt.rs:284](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/prompt.rs:284), [helpers.rs:338](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/helpers.rs:338), [host_value.rs:269](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/host_value.rs:269) | Prompt data can pass through InterpValue -> HostValue -> serde_json::Value -> String; secret traversal snapshots aggregate inputs; owned Json/Prompt values copy on local reads. Reduce intermediate graphs and borrow scans. Host encoding already borrows aggregate vectors, and protocol output ownership/redaction/bounds are mandatory. |
| O30 | Literal construction and evaluation descriptors: [aggregate_expr.rs](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/aggregate_expr.rs:5) | Known-size literals start with empty result vectors and clone HIR element/field descriptor lists for evaluation/resumption. Reserve capacity and share immutable descriptors. This is allocation overhead, not by itself quadratic execution. |
| O31 | Closure capture: [aggregate_expr.rs:428](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/aggregate_expr.rs:428) | Captures all frame locals, not only free variables. This copies unrelated owned payloads, retains otherwise dead containers, and increases Rc counts that later trigger COW. Capture checked free-variable slots and share immutable capture layouts. |

## Coverage Matrix

A dash means there is no separate local method implementation audited for that operation, not that the language guarantees it is supported.

| Family | Construction / update | Lookup / inspection | Equality / traversal / boundaries |
| --- | --- | --- | --- |
| Array | O01, O03, O24, O30 | direct indexed get is already borrowed; O12 | O23, O25, O27, O28 |
| List | O02, O03; G2 API gap | indexed access exists; head/tail API needs conformance coverage | O23, O25, O27, O28 |
| Slice | O12, O13 | direct indexed get is already borrowed | O23, O25, O27, O28 |
| Map | O08, O30 | O07; method len is O(1), pure ABI count path is O27 | O23, O27, O28; full iteration API not established here |
| Set | C2, O30 | no efficient keyed membership backing | O23, O25, O27, O28 |
| Deque | O04 | method len/is_empty are O(1) | O23 through ArrayValue; O25, O28; G1 |
| Queue | O05 | method len/is_empty are O(1) | O23 through ArrayValue; O25, O28; G1 |
| Stack | O06 | method len/is_empty are O(1) | O23 through ArrayValue; O25, O28; G1 |
| PriorityQueue | C1, O11 | method len/is_empty are O(1) | O23 through MapValue; O25, O28; G1 |
| OrderedMap | C3, O09 | O09 | O23, O25, O28; G1 reproduced |
| OrderedSet | C3, O10 | O10 | O23, O25, O28; G1 static omission |
| Range | O15 | endpoint representation is compact until expanded | O14, O15; typed ABI need not enumerate elements |
| string | O18, O19, O20 | O16, O17 | O27, O29; ordinary comparison need not allocate inside the comparison kernel |
| bytes | O12, O16 | owned local read copies before an otherwise simple lookup/count | O27, O29 |
| Record | O24, O30 | O21 | O22, O23, O27, O28 |
| Tuple | O26, O30 | payload clone at local read/selection | O26, O27, O28 |
| Option | O26 | tag inspection fast path already exists | O26, O27, O28 |
| Result | O26 | tag inspection fast path already exists | O26, O27, O28 |
| User enum | O22, O26 | checked variant/field identity must be retained | O22, O26, O27, O28 |
| Json | owned recursive representation | clone cost follows payload size | O16 principle, O29; checkpoint capture copies owned JSON |
| Prompt | O29; owned temporary append already reuses vector | channel assembly and secret checks | O29, O28; immutable publication/trace provenance must remain intact |

## Existing Useful Optimizations

Do not delete these while consolidating implementations:

- Rc-backed Array/List/Slice/Map/Set/Record clones share their backing; they are not all immediately deep-copied.
- Array/Slice get and the direct sequence index path borrow the backing and clone only the selected value.
- Local len/is_empty and several pure intrinsic count fast paths do not snapshot containers.
- Option/Result tag checks avoid converting the whole payload into BuiltinValue at the intrinsic dispatch stage. Earlier argument reads can still clone it.
- String addition consumes the left temporary and uses push_str.
- Pure ABI conversion moves already owned primitive String/Bytes rather than cloning them again.
- Host conversion borrows aggregate storage while building required protocol output.
- Standard string join uses a normal linear join implementation.
- Range loop materialization is limited by Iterations; it must not be confused with the unbounded original-range expansion used by slicing.

## Architecture Work Required

1. Make value storage own its invariants. Today mutable borrows and make_unique are exposed separately. The evaluator must not be the only place enforcing uniqueness, set uniqueness, sorted ordering or heap invariants.
2. Separate borrowed inspection, consuming transformation and shared-value transformation. Adding Rc around String alone does not make repeated reassignment linear when the frame retains an old alias during evaluation.
3. Use appropriate backing per collection. Vec for everything is not a generic container abstraction: List front operations, Deque, ordered collections and PriorityQueue require different algorithms.
4. Keep semantic layouts in checked facts/plan data: record slots, variant permutations, equality/order evidence, free-variable capture slots. Do not rediscover these by strings on every value access.
5. Share collection algorithms between the existing [values-argument implementation](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/local.rs:4) and [expression-argument implementation](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/collections.rs:3), including [advanced collections](/Users/hrtan/Projects/aplas/etas-interpreter/crates/etas_interpreter/src/eval/method/advanced_collections.rs:1). They are duplicate algorithm surfaces, not proven dead files. Keep one argument evaluation/resumption owner and one collection kernel per operation; audit callers before deleting entry points.
6. Maintain snapshot isolation, effect order, cancellation safe points, diagnostic spans, numeric checks, nominal identity and secret handling while optimizing. A builder is a distinct visible API requiring its own SPEC/registry/runtime contract, not an invisible mutable Array.
7. Memory retention matters as much as copies. A tiny slice or tokenized line can retain a huge backing buffer; define compaction/materialization choices rather than assuming every view is always cheaper.

For normal repeated local construction the target should be amortized linear total work where representation and ownership permit it. When old versions remain live, use structural sharing or account for necessary copying explicitly; destructive mutation is not permitted merely to meet a benchmark.

## Independent Test Evidence

The temporary audit harness at [container_review.rs](/tmp/etas-session-architect-audit/src/container_review.rs) compiles source through the current frontend and runs checked HIR through Interpreter. It uses in-memory host services and performs no network calls.

Command executed:

```sh
cargo test -q --offline --lib container_review -- --nocapture
```

Working directory: /tmp/etas-session-architect-audit.

Four regression probes failed as expected against the current implementation:
- priority_is_independent_of_insertion_order: two reversed insertion sequences return different first values (20 and 10).
- set_literal_is_unique: duplicate singleton set sums to 2 instead of 1.
- ordered_containers_iterate_by_key: rejected by the frontend before execution. Its later OrderedSet loop is not reached.
- ordered_containers_preserve_sorted_storage: returned OrderedMap and OrderedSet both contain [2, 1], not [1, 2]. Internal storage inspection supports C3 together with the unsorted iterator implementation; storage ordering alone is not a language conformance test.

These failures are not performance benchmarks. No repository tests were disabled or relaxed. The existing happy-path PriorityQueue test does not distinguish priority selection from LIFO when the higher-priority entry is inserted last.

## Required Performance And Regression Tests

- Add deterministic counters for outer elements copied, payload bytes copied, allocations and retained backing size. Wall-clock tests alone are noisy and cannot locate the source of copying.
- Measure checked-HIR execution separately from compilation/project loading; reuse the same prepared program for interpreter microbenchmarks.
- Exercise 1k/2k/4k workloads for push/extend/cons/front-pop, map lookup/update, concat, equality, field access and early-break iteration. Report scaling and copy counts rather than an unexplained timeout threshold.
- Cover unique temporaries, repeated binding reassignment, live aliases, nested updates, closures, checkpoints and resumable handlers separately.
- For borrowed reads/equality, verify no payload-sized copies and early length/mismatch exits where semantically valid.
- For Range slices, use huge logical ranges with tiny requested windows under controlled test resource limits. The expected algorithm must not enumerate the original range.
- Test Unicode multibyte text, unchanged trim, empty split/join input, empty pop, duplicate insertion and alias-preserving updates.
- Test PriorityQueue insertion permutations, equal-priority policy, ordered iteration, Set uniqueness, nested keys and checked comparison semantics. Do not infer ordering from rendering.
- Add checked-source conformance tests for every declared collection API and iterable family. Pure Rust storage tests cannot prove the frontend admits the public operation.
- Verify malformed checkpoint rejection, nominal/trust identity, storage limits and source evaluation order after ownership/ABI changes. Do not introduce a shape-based or unchecked fast path to pass a benchmark.
- Deeply nested owned ADT cloning/comparison/drop remains a recursion-risk area. This audit did not reproduce a stack overflow there; test it with controlled depth and process isolation before assigning a confirmed defect.

## Change Boundaries

This audit creates a review document only. It does not authorize an unreviewed container representation rewrite or new source-level builder API. No implementation, SPEC, EDK source or repository test was changed by this audit. Temporary probes are outside the repositories. No code was committed or pushed.
