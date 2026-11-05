#!/usr/bin/env python3
import argparse
import csv
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


DISPLAY_NAMES = {
    "history_compaction": "History\ncompaction",
    "tool_schema_pruning": "Tool schema\npruning",
    "observation_projection": "Observation\nprojection",
    "code_context_slicing": "Code context\nslicing",
    "all": "Overall",
}


def read_summary(path):
    with path.open(newline="") as handle:
        return list(csv.DictReader(handle))


def percent(numerator, denominator):
    return 100.0 * numerator / denominator if denominator else 0.0


def labeled_bars(ax, bars):
    ax.bar_label(bars, fmt="%.1f%%", padding=3, fontsize=9)


def plot(rows, output):
    labels = [DISPLAY_NAMES[row["category"]] for row in rows]
    x = list(range(len(rows)))
    width = 0.36

    prompt_reduction = [float(row["prompt_token_reduction_pct"]) for row in rows]
    total_reduction = [float(row["total_token_reduction_pct"]) for row in rows]
    elapsed_reduction = [float(row["elapsed_reduction_pct"]) for row in rows]
    baseline_quality = [float(row["baseline_quality_pct"]) for row in rows]
    optimized_quality = [float(row["optimized_quality_pct"]) for row in rows]
    baseline_cached_share = [
        percent(float(row["baseline_cached_tokens"]), float(row["baseline_prompt_tokens"]))
        for row in rows
    ]
    optimized_cached_share = [
        percent(float(row["optimized_cached_tokens"]), float(row["optimized_prompt_tokens"]))
        for row in rows
    ]

    plt.style.use("seaborn-v0_8-whitegrid")
    fig, axes = plt.subplots(2, 2, figsize=(16, 11), constrained_layout=True)

    ax = axes[0, 0]
    prompt_bars = ax.bar(
        [value - width / 2 for value in x],
        prompt_reduction,
        width,
        label="Prompt tokens",
        color="#2b6cb0",
    )
    total_bars = ax.bar(
        [value + width / 2 for value in x],
        total_reduction,
        width,
        label="Total tokens",
        color="#2f855a",
    )
    ax.set_xticks(x, labels)
    ax.set_ylim(0, 82)
    ax.set_ylabel("Reduction (%)")
    ax.set_title("A. Token reduction — higher is better")
    ax.legend(loc="upper right")
    labeled_bars(ax, prompt_bars)
    labeled_bars(ax, total_bars)

    ax = axes[0, 1]
    colors = ["#d69e2e" if value >= 0 else "#c53030" for value in elapsed_reduction]
    elapsed_bars = ax.bar(x, elapsed_reduction, color=colors)
    ax.axhline(0, color="#4a5568", linewidth=1)
    ax.set_xticks(x, labels)
    margin = max(5.0, max(abs(value) for value in elapsed_reduction) * 0.3)
    ax.set_ylim(min(0, min(elapsed_reduction)) - margin, max(elapsed_reduction) + margin)
    ax.set_ylabel("Elapsed-time reduction (%)")
    ax.set_title("B. Remote wall-clock reduction — positive is faster")
    labeled_bars(ax, elapsed_bars)

    ax = axes[1, 0]
    baseline_bars = ax.bar(
        [value - width / 2 for value in x],
        baseline_quality,
        width,
        label="Baseline",
        color="#718096",
    )
    optimized_bars = ax.bar(
        [value + width / 2 for value in x],
        optimized_quality,
        width,
        label="Optimized",
        color="#805ad5",
    )
    ax.set_xticks(x, labels)
    ax.set_ylim(90, 101.5)
    ax.set_ylabel("Quality pass rate (%)")
    ax.set_title("C. Exact-answer/tool quality gate")
    ax.legend(loc="lower right")
    labeled_bars(ax, baseline_bars)
    labeled_bars(ax, optimized_bars)

    ax = axes[1, 1]
    baseline_cache_bars = ax.bar(
        [value - width / 2 for value in x],
        baseline_cached_share,
        width,
        label="Baseline",
        color="#4a5568",
    )
    optimized_cache_bars = ax.bar(
        [value + width / 2 for value in x],
        optimized_cached_share,
        width,
        label="Optimized",
        color="#319795",
    )
    ax.set_xticks(x, labels)
    ax.set_ylim(0, 100)
    ax.set_ylabel("Cached share of prompt tokens (%)")
    ax.set_title("D. Provider cache share — reported separately")
    ax.legend(loc="upper right")
    labeled_bars(ax, baseline_cache_bars)
    labeled_bars(ax, optimized_cache_bars)

    overall = next(row for row in rows if row["category"] == "all")
    fig.suptitle(
        "MiniMax-M3.1-Flash Agent Context Optimization\n"
        f"{overall['scenarios']} scenarios · {overall['samples']} paired samples · "
        f"{2 * int(overall['samples'])} real API requests · "
        f"{overall['valid_pairs']} quality-valid pairs",
        fontsize=17,
        fontweight="bold",
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(output, dpi=180, bbox_inches="tight")
    plt.close(fig)


def main():
    repository = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--input",
        type=Path,
        default=repository / "artifacts/optimization/minimax_token_summary.csv",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=repository / "artifacts/optimization/minimax_token_experiments.png",
    )
    args = parser.parse_args()
    rows = read_summary(args.input.resolve())
    plot(rows, args.output.resolve())
    print(args.output.resolve())


if __name__ == "__main__":
    main()
