#!/usr/bin/env python3
import argparse
import csv
import subprocess
from collections import defaultdict
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np


PASS_FIXTURES = {
    "LowerOwnedSnapshotCow": "lower_owned_snapshot_cow",
    "FuseSerializationBoundary": "fuse_serialization_boundary",
    "SuffixDemandHistory": "suffix_demand_history",
    "PruneUnusedQualityContext": "quality_aware_context_policy",
    "PrepareVersionedRequest": "prepare_versioned_request",
    "VersionedPreparationCse": "versioned_preparation_cse",
    "FuseProjectionScans": "fuse_projection_scans",
    "BatchStorageSemijoin": "batch_storage_semijoin",
    "SynthesizeScopedIndex": "synthesize_scoped_index",
    "ScheduleBoundedPipeline": "schedule_bounded_pipeline",
    "LowerResourceRegion": "lower_resource_region",
}


def run(command, cwd):
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    return completed.stdout


def interpreter_metrics(repository):
    output = run(
        [
            "cargo",
            "test",
            "-p",
            "etas_interpreter",
            "--offline",
            "reduces_real",
            "--",
            "--nocapture",
            "--test-threads=1",
        ],
        repository,
    )
    rows = []
    policy_rows = []
    for line in output.splitlines():
        marker = line.find("ETAS_OPT_METRIC,")
        if marker >= 0:
            fields = line[marker:].split(",")
            if len(fields) == 11:
                rows.append(
                    {
                        "pass": fields[1],
                        "program": fields[2],
                        "baseline_level": fields[3],
                        "optimized_level": fields[4],
                        "baseline_steps": int(fields[5]),
                        "optimized_steps": int(fields[6]),
                        "baseline_allocations": int(fields[7]),
                        "optimized_allocations": int(fields[8]),
                        "baseline_elapsed_ns": int(fields[9]),
                        "optimized_elapsed_ns": int(fields[10]),
                    }
                )
        marker = line.find("ETAS_POLICY_METRIC,")
        if marker >= 0:
            fields = line[marker:].split(",")
            if len(fields) == 8:
                policy_rows.append(
                    {
                        "policy": fields[1],
                        "program": fields[2],
                        "level": fields[3],
                        "baseline_units": int(fields[4]),
                        "optimized_units": int(fields[5]),
                        "baseline_elapsed_ns": int(fields[6]),
                        "optimized_elapsed_ns": int(fields[7]),
                    }
                )
    if len(rows) != 110:
        raise RuntimeError(f"expected 110 optimization measurements, found {len(rows)}")
    if len(policy_rows) != 40:
        raise RuntimeError(f"expected 40 policy measurements, found {len(policy_rows)}")
    return rows, policy_rows


def frontend_metrics(repository):
    output = run(
        [
            "cargo",
            "bench",
            "-p",
            "etas_frontend",
            "--offline",
            "--bench",
            "optimization_levels",
        ],
        repository,
    )
    header = "level,fixtures,compile_ms,retained_bytes,hir_nodes,optimized_out_nodes,rewritten_nodes"
    lines = output.splitlines()
    start = lines.index(header)
    rows = list(csv.DictReader(lines[start : start + 5]))
    if len(rows) != 4:
        raise RuntimeError(f"expected four optimization levels, found {len(rows)}")
    return rows


def fixture_stats(repository):
    root = repository / "crates/etas_interpreter/fixtures/optimization"
    stats = {}
    for pass_name, directory in PASS_FIXTURES.items():
        fixtures = sorted((root / directory).glob("*.es"))
        stats[pass_name] = {
            "tests": len(fixtures),
            "lines": sum(len(path.read_text().splitlines()) for path in fixtures),
        }
    return stats


def reduction(baseline, optimized):
    return 100.0 * (sum(baseline) - sum(optimized)) / sum(baseline)


def write_csv(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=rows[0].keys(), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def plot(rows, policy_rows, levels, stats, output):
    grouped = defaultdict(list)
    for row in rows:
        grouped[row["pass"]].append(row)
    pass_names = list(PASS_FIXTURES)
    short_names = [
        name.removesuffix("Pass")
        .replace("Versioned", "Versioned\n")
        .replace("Serialization", "Serialization\n")
        .replace("QualityContext", "Quality\nContext")
        for name in pass_names
    ]
    steps = []
    allocations = []
    elapsed = []
    for name in pass_names:
        samples = grouped[name]
        steps.append(
            reduction(
                [row["baseline_steps"] for row in samples],
                [row["optimized_steps"] for row in samples],
            )
        )
        allocations.append(
            reduction(
                [row["baseline_allocations"] for row in samples],
                [row["optimized_allocations"] for row in samples],
            )
        )
        elapsed.append(
            reduction(
                [row["baseline_elapsed_ns"] for row in samples],
                [row["optimized_elapsed_ns"] for row in samples],
            )
        )

    plt.style.use("seaborn-v0_8-whitegrid")
    fig = plt.figure(figsize=(18, 13), constrained_layout=True)
    grid = fig.add_gridspec(2, 2, width_ratios=(1.45, 1.0))

    ax = fig.add_subplot(grid[0, 0])
    y = np.arange(len(pass_names))
    height = 0.24
    ax.barh(y - height, steps, height, label="Interpreter steps", color="#2b6cb0")
    ax.barh(y, allocations, height, label="Allocations", color="#2f855a")
    ax.barh(y + height, elapsed, height, label="Elapsed time", color="#d69e2e")
    ax.set_yticks(y, short_names)
    ax.invert_yaxis()
    ax.axvline(0, color="#444", linewidth=0.8)
    ax.set_xlabel("Reduction vs O0 (%) — higher is better")
    ax.set_title("A. Real execution benefit across 10 programs per pass")
    ax.legend(loc="lower right")

    ax = fig.add_subplot(grid[0, 1])
    lines = [stats[name]["lines"] for name in pass_names]
    bars = ax.barh(y, lines, color="#805ad5")
    ax.set_yticks(y, short_names)
    ax.invert_yaxis()
    ax.set_xlabel("Etas fixture source lines")
    ax.set_title("B. Test coverage size (110 programs total)")
    for bar, name in zip(bars, pass_names):
        ax.text(
            bar.get_width() + max(lines) * 0.01,
            bar.get_y() + bar.get_height() / 2,
            f'{stats[name]["tests"]} tests',
            va="center",
            fontsize=8,
        )

    ax = fig.add_subplot(grid[1, 0])
    level_names = [row["level"] for row in levels]
    compile_ms = [int(row["compile_ms"]) for row in levels]
    optimized_out = [int(row["optimized_out_nodes"]) for row in levels]
    bars = ax.bar(level_names, compile_ms, color="#3182ce", alpha=0.82)
    ax.set_ylabel("Compile time for 120 fixtures (ms)", color="#2b6cb0")
    ax.tick_params(axis="y", labelcolor="#2b6cb0")
    ax.set_title("C. Frontend optimization-level axis")
    for bar, value in zip(bars, compile_ms):
        ax.text(bar.get_x() + bar.get_width() / 2, value, str(value), ha="center", va="bottom")
    twin = ax.twinx()
    twin.plot(level_names, optimized_out, color="#c53030", marker="o", linewidth=2.5)
    twin.set_ylabel("HIR nodes optimized out", color="#c53030")
    twin.tick_params(axis="y", labelcolor="#c53030")

    ax = fig.add_subplot(grid[1, 1])
    policy_by_level = defaultdict(list)
    for row in policy_rows:
        policy_by_level[row["level"]].append(row)
    policy_levels = ["O0", "O1", "O2", "O3"]
    baseline_units = [
        np.mean([row["baseline_units"] for row in policy_by_level[level]])
        for level in policy_levels
    ]
    optimized_units = [
        np.mean([row["optimized_units"] for row in policy_by_level[level]])
        for level in policy_levels
    ]
    x = np.arange(4)
    ax.bar(x - 0.18, baseline_units, 0.36, label="Policy off", color="#718096")
    ax.bar(x + 0.18, optimized_units, 0.36, label="Token budget = 8", color="#dd6b20")
    ax.set_xticks(x, policy_levels)
    ax.set_ylabel("Mean model-input units")
    policy_reduction = reduction(
        [row["baseline_units"] for row in policy_rows],
        [row["optimized_units"] for row in policy_rows],
    )
    ax.set_title(f"D. Explicit policy kept separate ({policy_reduction:.1f}% fewer units)")
    ax.legend()

    total_lines = sum(item["lines"] for item in stats.values())
    fig.suptitle(
        f"Etas HIR Optimization Evidence — 110 programs, {total_lines} fixture LOC",
        fontsize=18,
        fontweight="bold",
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(output, dpi=180, bbox_inches="tight")


def main():
    repository = Path(__file__).resolve().parents[1]
    default_frontend = repository.parent / "etas-frontend-compiler-opt-levels"
    parser = argparse.ArgumentParser()
    parser.add_argument("--frontend", type=Path, default=default_frontend)
    parser.add_argument(
        "--output",
        type=Path,
        default=repository / "artifacts/optimization/optimization_overview.png",
    )
    args = parser.parse_args()

    rows, policy_rows = interpreter_metrics(repository)
    levels = frontend_metrics(args.frontend.resolve())
    stats = fixture_stats(repository)
    output = args.output.resolve()
    write_csv(output.parent / "interpreter_metrics.csv", rows)
    write_csv(output.parent / "policy_metrics.csv", policy_rows)
    write_csv(output.parent / "frontend_levels.csv", levels)
    plot(rows, policy_rows, levels, stats, output)
    print(output)


if __name__ == "__main__":
    main()
