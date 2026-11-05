#!/usr/bin/env python3
"""Measure agent-context optimizations against MiniMax-M3.1-Flash.

The API key is read at runtime and is never included in request/result artifacts.
"""

import argparse
import csv
import json
import random
import re
import threading
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path


ENDPOINT = "https://api.minimaxi.com/v1/chat/completions"
MODEL = "MiniMax-M3.1-Flash"
MAX_OUTPUT_TOKENS = 128
CATEGORIES = (
    "history_compaction",
    "tool_schema_pruning",
    "observation_projection",
    "code_context_slicing",
)
SAFE_DEFAULT_CATEGORIES = CATEGORIES[:-1]


@dataclass(frozen=True)
class Scenario:
    category: str
    scenario_id: str
    baseline: dict
    optimized: dict
    expected_kind: str
    expected_value: str


def answer_instruction(value):
    return f"Reply with exactly ANSWER:{value} and no other text."


def history_scenarios():
    facts = (
        ("chat_history", "hong-kong"),
        ("code_window", "scheduler.rs"),
        ("diagnostic_trace", "trace-73"),
        ("memory_recall", "checkpoint-9"),
        ("plan_history", "stage-compile"),
        ("policy_summary", "allow-read"),
        ("repo_map", "crates-agent"),
        ("retrieval_pack", "document-42"),
        ("session_summary", "session-blue"),
        ("tool_catalog", "workspace-search"),
    )
    scenarios = []
    for index, (name, current) in enumerate(facts):
        messages = [
            {
                "role": "system",
                "content": (
                    "You answer agent-state lookup questions. The CURRENT record is "
                    "authoritative; unrelated audit history is context only."
                ),
            },
            {
                "role": "user",
                "content": f"CURRENT {name} value: {current}. Remember it for the final query.",
            },
            {"role": "assistant", "content": f"Recorded CURRENT {name} as {current}."},
        ]
        for revision in range(1, 9):
            messages.extend(
                [
                    {
                        "role": "user",
                        "content": (
                            f"Audit event {revision}: worker={index + revision}; "
                            f"status=completed; latency_ms={80 + revision}; "
                            "diagnostics were archived and require no action."
                        ),
                    },
                    {
                        "role": "assistant",
                        "content": (
                            f"Audit event {revision} acknowledged. It does not replace the "
                            f"CURRENT {name} record."
                        ),
                    },
                ]
            )
        query = f"What is the CURRENT {name} value? {answer_instruction(current)}"
        messages.append({"role": "user", "content": query})
        optimized = {
            "messages": [
                messages[0],
                messages[1],
                messages[2],
                {"role": "user", "content": query},
            ]
        }
        scenarios.append(
            Scenario(
                "history_compaction",
                name,
                {"messages": messages},
                optimized,
                "answer",
                current,
            )
        )
    return scenarios


TOOL_SPECS = (
    ("file_search", "Find a workspace file by exact name", "name", "main.es"),
    ("memory_lookup", "Read an agent memory by search query", "query", "release gate"),
    ("policy_check", "Check whether an operation is allowed", "operation", "read workspace"),
    ("route_model", "Select a model route for a workload", "workload", "code review"),
    ("inspect_trace", "Inspect a trace using its identifier", "trace_id", "trace-73"),
    ("load_checkpoint", "Load a checkpoint using its identifier", "checkpoint_id", "checkpoint-9"),
    ("list_mcp_servers", "List MCP servers for a capability", "capability", "repository search"),
    ("read_workspace", "Read one workspace-relative source file", "path", "src/main.es"),
    ("validate_schema", "Validate a named JSON schema", "schema", "model-request-v2"),
    ("publish_result", "Publish a completed agent result", "result_id", "result-42"),
)


def tool_definition(spec):
    name, description, argument, _ = spec
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    argument: {
                        "type": "string",
                        "description": f"Required {argument.replace('_', ' ')}",
                    }
                },
                "required": [argument],
                "additionalProperties": False,
            },
        },
    }


def tool_schema_scenarios():
    all_tools = [tool_definition(spec) for spec in TOOL_SPECS]
    scenarios = []
    for spec in TOOL_SPECS:
        name, _, argument, value = spec
        prompt = (
            f"Use the {name} tool now with {argument} set to {json.dumps(value)}. "
            "Do not call any other tool and do not answer from memory."
        )
        common = {"messages": [{"role": "user", "content": prompt}], "tool_choice": "auto"}
        scenarios.append(
            Scenario(
                "tool_schema_pruning",
                name,
                {**common, "tools": all_tools},
                {**common, "tools": [tool_definition(spec)]},
                "tool",
                name,
            )
        )
    return scenarios


def observation_scenarios():
    facts = (
        ("retrieval", "chunk-18"),
        ("compiler", "hir-clean"),
        ("policy", "allow-write"),
        ("workspace", "branch-feature"),
        ("memory", "memory-v7"),
        ("checkpoint", "checkpoint-ready"),
        ("model_route", "route-flash"),
        ("tool_result", "exit-zero"),
        ("trace", "span-204"),
        ("session", "fence-31"),
    )
    scenarios = []
    for index, (name, value) in enumerate(facts):
        verbose = {
            "tool": f"agent_{name}_inspect",
            "request_id": f"request-{index + 100}",
            "attempts": [
                {
                    "attempt": attempt,
                    "worker": f"worker-{attempt % 3}",
                    "started_at": f"2026-11-05T10:{attempt:02d}:00+08:00",
                    "elapsed_ms": 120 + attempt * 17,
                    "cache": "miss" if attempt < 3 else "hit",
                    "log": (
                        "Transport completed normally; diagnostic payload retained for "
                        "post-run analysis; no operator action is required."
                    ),
                }
                for attempt in range(1, 7)
            ],
            "result": {"field": name, "current_value": value, "authoritative": True},
            "metadata": {
                "region": "cn-east-1",
                "runtime": "agent-host-v4",
                "serializer": "json-v2",
                "labels": ["optimization", "evaluation", "projection", "stable"],
            },
        }
        question = f"Read current_value from the observation. {answer_instruction(value)}"
        baseline_text = (
            "A tool returned this observation:\n"
            + json.dumps(verbose, ensure_ascii=False, indent=2)
            + "\n\n"
            + question
        )
        projected_text = (
            "A tool returned this projected observation:\n"
            + json.dumps({"current_value": value})
            + "\n\n"
            + question
        )
        scenarios.append(
            Scenario(
                "observation_projection",
                name,
                {"messages": [{"role": "user", "content": baseline_text}]},
                {"messages": [{"role": "user", "content": projected_text}]},
                "answer",
                value,
            )
        )
    return scenarios


def scenario_name(source, path):
    match = re.search(
        r'flow\s+scenario_name\s*\(\s*\)\s*->\s*string\s*\{\s*return\s+"([^"]+)"',
        source,
        re.DOTALL,
    )
    if match is None:
        raise RuntimeError(f"fixture has no literal scenario_name(): {path}")
    return match.group(1)


def code_context_scenarios(repository):
    root = repository / "crates/etas_interpreter/fixtures/optimization"
    targets = (
        root / "suffix_demand_history/last_user_message.es",
        root / "suffix_demand_history/latest_tool_result.es",
        root / "fuse_serialization_boundary/tool_schema.es",
        root / "fuse_serialization_boundary/model_payload.es",
        root / "prepare_versioned_request/prompt_static_segment.es",
        root / "prepare_versioned_request/provider_header_plan.es",
        root / "lower_owned_snapshot_cow/session_state.es",
        root / "lower_owned_snapshot_cow/mcp_catalog.es",
        root / "lower_resource_region/model_client.es",
        root / "lower_resource_region/tool_host.es",
    )
    sources = [(path, path.read_text()) for path in targets]
    scenarios = []
    for index, (target, target_source) in enumerate(sources):
        expected = scenario_name(target_source, target)
        distractors = [sources[(index + offset) % len(sources)] for offset in range(1, 5)]

        def context(entries):
            return "\n\n".join(
                f"===== FILE {path.relative_to(repository)} =====\n{source}"
                for path, source in entries
            )

        question = (
            f"In the TARGET file {target.relative_to(repository)}, what exact string literal "
            f"does scenario_name() return? {answer_instruction(expected)}"
        )
        baseline = context([(target, target_source), *distractors]) + "\n\n" + question
        optimized = context([(target, target_source)]) + "\n\n" + question
        scenarios.append(
            Scenario(
                "code_context_slicing",
                target.stem,
                {"messages": [{"role": "user", "content": baseline}]},
                {"messages": [{"role": "user", "content": optimized}]},
                "answer",
                expected,
            )
        )
    return scenarios


def build_scenarios(repository, categories):
    builders = {
        "history_compaction": history_scenarios,
        "tool_schema_pruning": tool_schema_scenarios,
        "observation_projection": observation_scenarios,
        "code_context_slicing": lambda: code_context_scenarios(repository),
    }
    scenarios = []
    for category in CATEGORIES:
        if category in categories:
            category_scenarios = builders[category]()
            if len(category_scenarios) != 10:
                raise RuntimeError(
                    f"expected 10 {category} scenarios, found {len(category_scenarios)}"
                )
            scenarios.extend(category_scenarios)
    return scenarios


class MiniMaxClient:
    def __init__(self, api_key, timeout_seconds=90, request_interval=0.5):
        self.api_key = api_key
        self.timeout_seconds = timeout_seconds
        self.request_interval = request_interval
        self.rate_lock = threading.Lock()
        self.next_request_at = 0.0

    def wait_for_slot(self):
        with self.rate_lock:
            now = time.monotonic()
            wait_seconds = max(0.0, self.next_request_at - now)
            if wait_seconds:
                time.sleep(wait_seconds)
            self.next_request_at = time.monotonic() + self.request_interval

    def complete(self, request_body):
        payload = {
            "model": MODEL,
            "temperature": 0,
            "max_tokens": MAX_OUTPUT_TOKENS,
            **request_body,
        }
        request = urllib.request.Request(
            ENDPOINT,
            data=json.dumps(payload).encode(),
            headers={
                "Authorization": f"Bearer {self.api_key}",
                "Content-Type": "application/json",
            },
        )
        for attempt in range(6):
            self.wait_for_slot()
            started = time.perf_counter_ns()
            try:
                with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                    body = json.load(response)
                return body, (time.perf_counter_ns() - started) / 1_000_000
            except urllib.error.HTTPError as error:
                if error.code not in (429, 500, 502, 503, 504) or attempt == 5:
                    raise
                retry_after = error.headers.get("Retry-After")
                delay = float(retry_after) if retry_after else min(30, 2**attempt)
                time.sleep(delay)
            except (TimeoutError, urllib.error.URLError):
                if attempt == 5:
                    raise
                time.sleep(min(30, 2**attempt))
        raise AssertionError("retry loop exhausted")


def observed_value(message, kind):
    if kind == "tool":
        calls = message.get("tool_calls") or []
        return calls[0]["function"]["name"] if calls else ""
    content = message.get("content") or ""
    match = re.search(r"ANSWER:\s*([A-Za-z0-9_.-]+)", content)
    return match.group(1) if match else content.strip()


def execute_variant(client, scenario, variant, repetition):
    body = scenario.baseline if variant == "baseline" else scenario.optimized
    response, elapsed_ms = client.complete(body)
    choice = response["choices"][0]
    message = choice["message"]
    usage = response["usage"]
    observed = observed_value(message, scenario.expected_kind)
    prompt_details = usage.get("prompt_tokens_details") or {}
    completion_details = usage.get("completion_tokens_details") or {}
    return {
        "category": scenario.category,
        "scenario_id": scenario.scenario_id,
        "repetition": repetition,
        "variant": variant,
        "model": response.get("model", MODEL),
        "finish_reason": choice.get("finish_reason", ""),
        "expected_kind": scenario.expected_kind,
        "expected_value": scenario.expected_value,
        "observed_value": observed,
        "quality_pass": int(observed == scenario.expected_value),
        "prompt_tokens": int(usage["prompt_tokens"]),
        "cached_tokens": int(prompt_details.get("cached_tokens", 0)),
        "completion_tokens": int(usage["completion_tokens"]),
        "reasoning_tokens": int(completion_details.get("reasoning_tokens", 0)),
        "total_tokens": int(usage["total_tokens"]),
        "elapsed_ms": round(elapsed_ms, 3),
    }


def execute_pair(client, scenario, repetition):
    variants = (
        ("baseline", "optimized")
        if repetition % 2
        else ("optimized", "baseline")
    )
    rows = [execute_variant(client, scenario, variant, repetition) for variant in variants]
    return sorted(rows, key=lambda row: row["variant"])


def reduction_percent(baseline, optimized):
    if baseline == 0:
        return 0.0
    return 100.0 * (baseline - optimized) / baseline


def summarize(rows):
    summary = []
    for category in (*CATEGORIES, "all"):
        selected = rows if category == "all" else [row for row in rows if row["category"] == category]
        if not selected:
            continue
        by_variant = {
            variant: [row for row in selected if row["variant"] == variant]
            for variant in ("baseline", "optimized")
        }
        baseline_prompt = sum(row["prompt_tokens"] for row in by_variant["baseline"])
        optimized_prompt = sum(row["prompt_tokens"] for row in by_variant["optimized"])
        baseline_total = sum(row["total_tokens"] for row in by_variant["baseline"])
        optimized_total = sum(row["total_tokens"] for row in by_variant["optimized"])
        baseline_elapsed = sum(row["elapsed_ms"] for row in by_variant["baseline"])
        optimized_elapsed = sum(row["elapsed_ms"] for row in by_variant["optimized"])
        sample_count = len(by_variant["baseline"])
        scenario_count = len({row["scenario_id"] for row in selected})
        summary.append(
            {
                "category": category,
                "scenarios": scenario_count,
                "samples": sample_count,
                "valid_pairs": sum(
                    1
                    for baseline, optimized in zip(
                        by_variant["baseline"], by_variant["optimized"]
                    )
                    if baseline["quality_pass"] and optimized["quality_pass"]
                ),
                "baseline_prompt_tokens": baseline_prompt,
                "optimized_prompt_tokens": optimized_prompt,
                "prompt_token_reduction_pct": round(
                    reduction_percent(baseline_prompt, optimized_prompt), 3
                ),
                "baseline_total_tokens": baseline_total,
                "optimized_total_tokens": optimized_total,
                "total_token_reduction_pct": round(
                    reduction_percent(baseline_total, optimized_total), 3
                ),
                "baseline_cached_tokens": sum(
                    row["cached_tokens"] for row in by_variant["baseline"]
                ),
                "optimized_cached_tokens": sum(
                    row["cached_tokens"] for row in by_variant["optimized"]
                ),
                "elapsed_reduction_pct": round(
                    reduction_percent(baseline_elapsed, optimized_elapsed), 3
                ),
                "baseline_quality_pct": round(
                    100.0
                    * sum(row["quality_pass"] for row in by_variant["baseline"])
                    / sample_count,
                    3,
                ),
                "optimized_quality_pct": round(
                    100.0
                    * sum(row["quality_pass"] for row in by_variant["optimized"])
                    / sample_count,
                    3,
                ),
            }
        )
    return summary


def write_csv(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=rows[0].keys(), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def read_csv(path):
    integer_fields = {
        "repetition",
        "quality_pass",
        "prompt_tokens",
        "cached_tokens",
        "completion_tokens",
        "reasoning_tokens",
        "total_tokens",
    }
    with path.open(newline="") as handle:
        rows = list(csv.DictReader(handle))
    for row in rows:
        row.setdefault("repetition", "1")
        for field in integer_fields:
            row[field] = int(row[field])
        row["elapsed_ms"] = float(row["elapsed_ms"])
    return rows


def parse_args(repository):
    parser = argparse.ArgumentParser()
    parser.add_argument("--key-file", type=Path, default=Path.home() / "api/minimax")
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=repository / "artifacts/optimization",
    )
    parser.add_argument("--category", action="append", choices=CATEGORIES)
    parser.add_argument(
        "--include-repository-context",
        action="store_true",
        help="Allow code_context_slicing prompts to contain repository fixture source.",
    )
    parser.add_argument(
        "--limit",
        type=int,
        help="Run at most this many scenarios per selected category.",
    )
    parser.add_argument("--max-workers", type=int, choices=(1, 2), default=1)
    parser.add_argument("--repetitions", type=int, default=1)
    parser.add_argument("--request-interval", type=float, default=0.5)
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--seed", type=int, default=20261105)
    return parser.parse_args()


def main():
    repository = Path(__file__).resolve().parents[1]
    args = parse_args(repository)
    if args.repetitions < 1:
        raise SystemExit("--repetitions must be at least 1")
    if args.request_interval < 0:
        raise SystemExit("--request-interval must not be negative")
    categories = set(args.category or SAFE_DEFAULT_CATEGORIES)
    if "code_context_slicing" in categories and not args.include_repository_context:
        raise SystemExit(
            "code_context_slicing sends repository fixture source to MiniMax; "
            "pass --include-repository-context only after this export is authorized"
        )
    scenarios = build_scenarios(repository, categories)
    if args.limit is not None:
        scenarios = [
            scenario
            for category in CATEGORIES
            for scenario in [item for item in scenarios if item.category == category][: args.limit]
        ]
    random.Random(args.seed).shuffle(scenarios)
    output_dir = args.output_dir.resolve()
    partial_path = output_dir / "minimax_token_experiments.partial.csv"
    final_path = output_dir / "minimax_token_experiments.csv"
    resume_path = partial_path if partial_path.exists() else final_path
    rows = read_csv(resume_path) if args.resume and resume_path.exists() else []
    completed_keys = {
        (row["category"], row["scenario_id"], row["repetition"])
        for row in rows
    }
    api_key = args.key_file.read_text().strip()
    client = MiniMaxClient(api_key, request_interval=args.request_interval)
    completed = 0
    lock = threading.Lock()
    work = [
        (scenario, repetition)
        for repetition in range(1, args.repetitions + 1)
        for scenario in scenarios
        if (scenario.category, scenario.scenario_id, repetition) not in completed_keys
    ]
    with ThreadPoolExecutor(max_workers=args.max_workers) as executor:
        futures = {
            executor.submit(execute_pair, client, scenario, repetition): (
                scenario,
                repetition,
            )
            for scenario, repetition in work
        }
        for future in as_completed(futures):
            pair = future.result()
            with lock:
                rows.extend(pair)
                completed += 1
                scenario, repetition = futures[future]
                rows.sort(
                    key=lambda row: (
                        row["category"],
                        row["scenario_id"],
                        row["repetition"],
                        row["variant"],
                    )
                )
                write_csv(partial_path, rows)
                print(
                    f"[{completed:03d}/{len(work):03d}] "
                    f"{scenario.category}/{scenario.scenario_id}#{repetition}: "
                    f"quality={pair[0]['quality_pass']}/{pair[1]['quality_pass']} "
                    f"prompt={pair[0]['prompt_tokens']}->{pair[1]['prompt_tokens']}",
                    flush=True,
                )
    rows.sort(
        key=lambda row: (
            row["category"],
            row["scenario_id"],
            row["repetition"],
            row["variant"],
        )
    )
    summary = summarize(rows)
    write_csv(final_path, rows)
    write_csv(output_dir / "minimax_token_summary.csv", summary)
    if partial_path.exists():
        partial_path.unlink()
    print(json.dumps(summary, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
