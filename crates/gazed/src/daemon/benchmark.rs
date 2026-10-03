// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) const BENCHMARK_WARMUP_ITERS: usize = 3;
pub(super) const BENCHMARK_TIMED_ITERS: usize = 15;

pub(super) fn benchmark_component(
    component: &str,
    runtime: &gaze_vision::inference::InferenceRuntime,
    mut run_once: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<gaze_core::dbus::BenchmarkResult> {
    for _ in 0..BENCHMARK_WARMUP_ITERS {
        run_once()?;
    }

    let mut samples_ms = Vec::with_capacity(BENCHMARK_TIMED_ITERS);
    for _ in 0..BENCHMARK_TIMED_ITERS {
        let start = Instant::now();
        run_once()?;
        samples_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples_ms.sort_by(f64::total_cmp);

    let mean_ms = samples_ms.iter().sum::<f64>() / samples_ms.len() as f64;
    let min_ms = samples_ms[0];
    let p95_idx = (((samples_ms.len() - 1) as f64) * 0.95).round() as usize;
    let p95_ms = samples_ms[p95_idx];
    let fps = if mean_ms > 0.0 { 1000.0 / mean_ms } else { 0.0 };

    Ok(gaze_core::dbus::BenchmarkResult {
        component: component.to_string(),
        execution_provider: runtime.active_execution_provider.clone(),
        device: runtime.active_device.clone(),
        requested_execution_provider: runtime.requested_execution_provider.clone(),
        requested_device: runtime.requested_device.clone(),
        fallback_reason: runtime.fallback_reason.clone().unwrap_or_default(),
        mean_ms,
        p95_ms,
        min_ms,
        fps,
    })
}

pub(super) fn run_inference_benchmark(
    detector: Arc<std::sync::Mutex<FaceDetector>>,
    recognizer_rgb: Arc<Mutex<FaceRecognizer>>,
    recognizer_ir: Arc<Mutex<FaceRecognizer>>,
    liveness: Arc<Mutex<Option<LivenessDetector>>>,
) -> fdo::Result<Vec<gaze_core::dbus::BenchmarkResult>> {
    let mut results = Vec::new();

    {
        let mut detector = detector.lock().unwrap_or_else(|e| e.into_inner());
        let runtime = detector.inference_runtime().clone();
        let result =
            benchmark_component(
                "Face detector",
                &runtime,
                || Ok(detector.benchmark_infer()?),
            )
            .map_err(|e| fdo::Error::Failed(format!("detector benchmark failed: {e}")))?;
        results.push(result);
    }

    let synthetic_face = image::RgbImage::from_pixel(112, 112, image::Rgb([128, 128, 128]));

    {
        let mut recognizer = recognizer_rgb.blocking_lock();
        let runtime = recognizer.inference_runtime().clone();
        let result = benchmark_component("Face recognizer (RGB)", &runtime, || {
            recognizer.get_embedding(&synthetic_face).map(|_| ())
        })
        .map_err(|e| fdo::Error::Failed(format!("RGB recognizer benchmark failed: {e}")))?;
        results.push(result);
    }

    {
        let mut recognizer = recognizer_ir.blocking_lock();
        let runtime = recognizer.inference_runtime().clone();
        let result = benchmark_component("Face recognizer (IR)", &runtime, || {
            recognizer.get_embedding(&synthetic_face).map(|_| ())
        })
        .map_err(|e| fdo::Error::Failed(format!("IR recognizer benchmark failed: {e}")))?;
        results.push(result);
    }

    {
        let mut liveness_guard = liveness.blocking_lock();
        if let Some(detector) = liveness_guard.as_mut() {
            let runtime = detector.inference_runtime().clone();
            let result = benchmark_component("Liveness (MiniFASNet)", &runtime, || {
                detector.live_score(&synthetic_face).map(|_| ())
            })
            .map_err(|e| fdo::Error::Failed(format!("liveness benchmark failed: {e}")))?;
            results.push(result);
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::{BENCHMARK_TIMED_ITERS, BENCHMARK_WARMUP_ITERS, benchmark_component};
    use gaze_vision::inference::InferenceRuntime;

    fn cpu_runtime() -> InferenceRuntime {
        InferenceRuntime {
            requested_execution_provider: "cpu".to_string(),
            requested_device: "cpu".to_string(),
            active_execution_provider: "cpu".to_string(),
            active_device: "cpu".to_string(),
            fallback_reason: None,
        }
    }

    #[test]
    fn runs_warmup_then_timed_iterations_and_reports_ordered_stats() {
        let calls = std::cell::Cell::new(0usize);
        let runtime = cpu_runtime();
        let result = benchmark_component("Test model", &runtime, || {
            calls.set(calls.get() + 1);
            Ok(())
        })
        .unwrap();

        assert_eq!(calls.get(), BENCHMARK_WARMUP_ITERS + BENCHMARK_TIMED_ITERS);
        assert_eq!(result.component, "Test model");
        assert!(result.ran_as_configured());
        assert!(result.min_ms <= result.mean_ms);
        assert!(result.min_ms <= result.p95_ms);
        assert!(result.fps >= 0.0);
    }

    #[test]
    fn propagates_the_first_error_from_warmup() {
        let runtime = cpu_runtime();
        let err =
            benchmark_component("Failing model", &runtime, || anyhow::bail!("boom")).unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn reports_the_fallback_when_the_requested_device_is_not_in_use() {
        let runtime = InferenceRuntime {
            requested_execution_provider: "openvino".to_string(),
            requested_device: "npu".to_string(),
            active_execution_provider: "cpu".to_string(),
            active_device: "cpu".to_string(),
            fallback_reason: Some("no npu driver".to_string()),
        };
        let result = benchmark_component("Test model", &runtime, || Ok(())).unwrap();

        assert!(!result.ran_as_configured());
        assert_eq!(result.execution_provider, "cpu");
        assert_eq!(result.requested_device, "npu");
        assert_eq!(result.fallback_reason, "no npu driver");
    }
}
