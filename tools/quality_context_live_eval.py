#!/usr/bin/env python3
import argparse
import json
import re
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path


CASES = [
    ("repo_map", "What is the repository entry module?", ("src", "main")),
    ("chat_history", "Which deployment region did the user request?", ("hong", "kong")),
    ("retrieval_pack", "What language and source-file kind does the project use?", ("rust", "etas")),
    ("session_summary", "What is the active objective?", ("typed", "hir", "optimization", "passes")),
    ("tool_catalog", "Which tools are currently available?", ("read", "edit", "test", "git")),
    ("diagnostic_trace", "What is the final diagnostic?", ("effect", "row", "mismatch")),
    ("code_window", "What does the nearby target function return?", ("optimization", "report", "rewritten", "count")),
    ("memory_recall", "What answer style does the user prefer?", ("concise", "chinese")),
    ("plan_history", "What is the current plan?", ("full", "test", "merge", "main")),
    ("policy_summary", "What does the final policy require?", ("approval", "external", "destructive", "actions")),
]


def fixture_context(root: Path, name: str) -> str:
    source = (
        root
        / "crates/etas_interpreter/fixtures/optimization/quality_aware_context_policy_budget"
        / f"{name}.es"
    ).read_text()
    return re.search(
        r"let\s+(?:context|history|retrieval|summary)_[^=]+\s*=\s*\"([^\"]+)\"",
        source,
    ).group(1)


def normalized(text: str) -> str:
    return " ".join(re.findall(r"[a-z0-9]+", text.lower()))


def request(endpoint: str, api_key: str, model: str, context: str, question: str) -> dict:
    payload = json.dumps(
        {
            "model": model,
            "messages": [
                {
                    "role": "system",
                    "content": "Answer only from the supplied context. Return only the short answer in English.",
                },
                {
                    "role": "user",
                    "content": f"Context: {context}\nQuestion: {question}",
                },
            ],
            "temperature": 0,
            "max_completion_tokens": 256,
            "reasoning_split": True,
            "reasoning_effort": "low",
        }
    ).encode()
    http_request = urllib.request.Request(
        endpoint,
        data=payload,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    started = time.perf_counter()
    with urllib.request.urlopen(http_request, timeout=120) as response:
        body = json.load(response)
    return {
        "answer": body["choices"][0]["message"]["content"],
        "prompt_tokens": body["usage"]["prompt_tokens"],
        "completion_tokens": body["usage"]["completion_tokens"],
        "latency_ms": round((time.perf_counter() - started) * 1000, 3),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--api-key-file", default="/home/tanhuiri/api/minimax")
    parser.add_argument("--endpoint", default="https://api.minimaxi.com/v1/text/chatcompletion_v2")
    parser.add_argument("--model", default="MiniMax-M3.1-Flash")
    parser.add_argument("--budget", type=int, default=8)
    parser.add_argument("--case")
    parser.add_argument("--output", default="docs/quality-context-live-eval.json")
    args = parser.parse_args()

    root = Path(__file__).resolve().parent.parent
    api_key = Path(args.api_key_file).read_text().strip()
    results = []
    cases = [case for case in CASES if args.case is None or case[0] == args.case]
    for name, question, expected in cases:
        baseline_context = fixture_context(root, name)
        optimized_context = " ".join(baseline_context.split()[-args.budget :])
        baseline = request(args.endpoint, api_key, args.model, baseline_context, question)
        optimized = request(args.endpoint, api_key, args.model, optimized_context, question)
        baseline_answer = baseline.pop("answer")
        optimized_answer = optimized.pop("answer")
        baseline_normalized = normalized(baseline_answer)
        optimized_normalized = normalized(optimized_answer)
        baseline_ok = all(normalized(fact) in baseline_normalized for fact in expected)
        optimized_ok = all(normalized(fact) in optimized_normalized for fact in expected)
        assert baseline_ok and optimized_ok, (
            f"{name}: expected fact was not retained; "
            f"baseline={baseline_answer!r}; optimized={optimized_answer!r}"
        )
        assert optimized["prompt_tokens"] < baseline["prompt_tokens"], (
            f"{name}: prompt tokens did not decrease"
        )
        results.append(
            {
                "case": name,
                "expected_fact_retained": True,
                "baseline": baseline,
                "optimized": optimized,
            }
        )
        print(
            f"{name}: prompt tokens {baseline['prompt_tokens']} -> "
            f"{optimized['prompt_tokens']}; latency {baseline['latency_ms']} -> "
            f"{optimized['latency_ms']} ms"
        )

    report = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "model": args.model,
        "endpoint": args.endpoint,
        "context_unit_budget": args.budget,
        "cases": results,
        "aggregate": {
            "cases": len(results),
            "fact_retention_passes": sum(case["expected_fact_retained"] for case in results),
            "baseline_prompt_tokens": sum(
                case["baseline"]["prompt_tokens"] for case in results
            ),
            "optimized_prompt_tokens": sum(
                case["optimized"]["prompt_tokens"] for case in results
            ),
            "baseline_latency_ms": round(
                sum(case["baseline"]["latency_ms"] for case in results), 3
            ),
            "optimized_latency_ms": round(
                sum(case["optimized"]["latency_ms"] for case in results), 3
            ),
        },
    }
    output = root / args.output
    output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(f"wrote {output}")


if __name__ == "__main__":
    main()
