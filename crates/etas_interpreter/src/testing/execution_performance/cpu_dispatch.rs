use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{FakeHost, checked_project};
use crate::api::{
    CpuTaskTelemetry, EntryPoint, InterpValue, RunEventObserver, RunOptions, WorkflowEvent,
};

#[derive(Debug, Default)]
struct Metrics(Mutex<Vec<CpuTaskTelemetry>>);

impl RunEventObserver for Metrics {
    fn observe(&self, _: &WorkflowEvent) {}
    fn observe_cpu(&self, telemetry: &CpuTaskTelemetry) {
        self.0.lock().unwrap().push(telemetry.clone());
    }
}

fn median(values: &mut [Duration]) -> f64 {
    values.sort_unstable();
    values[values.len() / 2].as_secs_f64() * 1_000_000.0
}

// Keep this a manual release benchmark: correctness tests never assert a speedup
// from a noisy wall-clock sample. CPU_MIN_WORK=0 measures the offload crossover.
#[test]
#[ignore = "cargo test --release cpu_dispatch_benchmark -- --ignored --nocapture --test-threads=1"]
fn cpu_dispatch_benchmark() {
    let samples = std::env::var("ETAS_CPU_BENCH_SAMPLES")
        .map(|v| v.parse::<usize>().unwrap())
        .unwrap_or(3);
    assert!(samples > 0);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let host = FakeHost::new(Default::default());
    for shape in ["map", "join", "nested"] {
        for count in [1, 4, 32, 1000] {
            for (size, iterations) in [("micro", 0), ("medium", 128), ("heavy", 2048)] {
                let body = if iterations == 0 {
                    "return value + 1;".to_owned()
                } else {
                    format!(
                        "var index = 0; while index < {iterations} limit Iterations({iterations}) {{ index = index + 1; }} return value + 1;"
                    )
                };
                let work = match shape {
                    "map" => "map_concurrent(values, burn)".to_owned(),
                    "join" => format!("join([{}])", vec!["job"; count].join(",")),
                    "nested" => {
                        "run((inner: Scheduler) => { return map_concurrent(values, burn); })"
                            .to_owned()
                    }
                    _ => unreachable!(),
                };
                let checked = checked_project(&format!(
                    r#"
module app.main;
import std.task.{{map_concurrent, join, run, Scheduler}};
flow burn(value: i32) -> i32 {{ {body} }}
flow job() -> i32 {{ return burn(0); }}
flow main(values: Array<i32>) -> Array<i32> {{
    return run((scheduler: Scheduler) => {{ return {work}; }});
}}
"#
                ));
                let expected = InterpValue::Array(
                    (0..count)
                        .map(|index| {
                            InterpValue::i32(if shape == "join" { 1 } else { index as i32 + 1 })
                        })
                        .collect::<Vec<_>>()
                        .into(),
                );
                let mut baseline = 0.0;
                for workers in [1, 2, 4, 8] {
                    let mut wall = Vec::new();
                    let mut metrics = Vec::new();
                    for round in 0..=samples {
                        let observer = Arc::new(Metrics::default());
                        let mut options = RunOptions::default();
                        options.cpu_threads = std::num::NonZeroUsize::new(workers).unwrap();
                        options.cpu_min_work = 0;
                        options.event_observer = Some(observer.clone());
                        let args = vec![InterpValue::Array(
                            (0..count)
                                .map(|value| InterpValue::i32(value as i32))
                                .collect::<Vec<_>>()
                                .into(),
                        )];
                        let started = Instant::now();
                        let result = runtime
                            .block_on(crate::Interpreter.run_checked(
                                &checked,
                                EntryPoint {
                                    item: checked.entry.unwrap(),
                                },
                                args,
                                &host,
                                options,
                            ))
                            .unwrap();
                        let elapsed = started.elapsed();
                        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
                        assert_eq!(result.value(), Some(&expected));
                        if workers > 1 {
                            assert_eq!(observer.0.lock().unwrap().len(), count);
                        }
                        if round > 0 {
                            wall.push(elapsed);
                            metrics.extend(observer.0.lock().unwrap().drain(..));
                        }
                    }
                    let wall_us = median(&mut wall);
                    if workers == 1 {
                        baseline = wall_us;
                    }
                    let phase = |get: fn(&CpuTaskTelemetry) -> Duration| {
                        if metrics.is_empty() {
                            return 0.0;
                        }
                        median(&mut metrics.iter().map(get).collect::<Vec<_>>())
                    };
                    println!(
                        "{}",
                        serde_json::json!({
                            "benchmark": "cpu_dispatch", "shape": shape, "tasks": count, "size": size,
                            "iterations": iterations, "workers": workers, "warmup": 1, "samples": samples,
                            "policy": "all_eligible_calibration", "wall_median_us": wall_us,
                            "speedup": baseline / wall_us,
                            "task_median_us": {
                                "input_freeze": phase(|m| m.timings.input_freeze),
                                "program_prepare": phase(|m| m.timings.program_prepare),
                                "queue_wait": phase(|m| m.timings.queue_wait),
                                "input_thaw": phase(|m| m.timings.input_thaw),
                                "bootstrap": phase(|m| m.timings.bootstrap),
                                "execution": phase(|m| m.timings.execution),
                                "output_freeze": phase(|m| m.timings.output_freeze),
                                "output_thaw": phase(|m| m.timings.output_thaw),
                                "return_wait": phase(|m| m.timings.return_wait),
                            },
                        })
                    );
                }
            }
        }
    }
}
