#!/usr/bin/env python3
import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("run_minimax_token_experiments.py")
SPEC = importlib.util.spec_from_file_location("minimax_experiments", SCRIPT)
EXPERIMENTS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EXPERIMENTS)


class ScenarioTests(unittest.TestCase):
    def test_safe_default_builds_ten_scenarios_per_category(self):
        repository = SCRIPT.resolve().parents[1]
        scenarios = EXPERIMENTS.build_scenarios(
            repository, set(EXPERIMENTS.SAFE_DEFAULT_CATEGORIES)
        )
        counts = {
            category: sum(scenario.category == category for scenario in scenarios)
            for category in EXPERIMENTS.SAFE_DEFAULT_CATEGORIES
        }
        self.assertEqual(counts, {category: 10 for category in counts})

    def test_repository_context_builds_from_real_fixtures(self):
        repository = SCRIPT.resolve().parents[1]
        scenarios = EXPERIMENTS.code_context_scenarios(repository)
        self.assertEqual(len(scenarios), 10)
        self.assertTrue(
            all("===== FILE crates/etas_interpreter/fixtures" in item.baseline["messages"][0]["content"] for item in scenarios)
        )
        self.assertTrue(
            all(
                len(item.baseline["messages"][0]["content"])
                > len(item.optimized["messages"][0]["content"])
                for item in scenarios
            )
        )


class ResultTests(unittest.TestCase):
    def test_extracts_answer_and_tool_observations(self):
        self.assertEqual(
            EXPERIMENTS.observed_value({"content": "ANSWER:checkpoint-9"}, "answer"),
            "checkpoint-9",
        )
        self.assertEqual(
            EXPERIMENTS.observed_value(
                {
                    "tool_calls": [
                        {"function": {"name": "memory_lookup", "arguments": "{}"}}
                    ]
                },
                "tool",
            ),
            "memory_lookup",
        )

    def test_csv_round_trip_preserves_metric_types(self):
        row = {
            "category": "history_compaction",
            "scenario_id": "chat_history",
            "repetition": 2,
            "variant": "optimized",
            "model": EXPERIMENTS.MODEL,
            "finish_reason": "stop",
            "expected_kind": "answer",
            "expected_value": "current",
            "observed_value": "current",
            "quality_pass": 1,
            "prompt_tokens": 100,
            "cached_tokens": 64,
            "completion_tokens": 8,
            "reasoning_tokens": 0,
            "total_tokens": 108,
            "elapsed_ms": 12.5,
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.csv"
            EXPERIMENTS.write_csv(path, [row])
            restored = EXPERIMENTS.read_csv(path)
        self.assertEqual(restored, [row])

    def test_summary_uses_percentages_and_quality_pairs(self):
        common = {
            "category": "history_compaction",
            "scenario_id": "chat_history",
            "repetition": 1,
            "model": EXPERIMENTS.MODEL,
            "finish_reason": "stop",
            "expected_kind": "answer",
            "expected_value": "current",
            "observed_value": "current",
            "cached_tokens": 0,
            "completion_tokens": 10,
            "reasoning_tokens": 0,
            "elapsed_ms": 100.0,
        }
        rows = [
            {
                **common,
                "variant": "baseline",
                "quality_pass": 1,
                "prompt_tokens": 200,
                "total_tokens": 210,
            },
            {
                **common,
                "variant": "optimized",
                "quality_pass": 1,
                "prompt_tokens": 100,
                "total_tokens": 110,
                "elapsed_ms": 80.0,
            },
        ]
        summary = EXPERIMENTS.summarize(rows)[0]
        self.assertEqual(summary["valid_pairs"], 1)
        self.assertEqual(summary["prompt_token_reduction_pct"], 50.0)
        self.assertEqual(summary["elapsed_reduction_pct"], 20.0)


if __name__ == "__main__":
    unittest.main()
