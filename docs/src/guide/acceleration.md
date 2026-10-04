<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Hardware acceleration

The standard Gaze daemon supports Intel OpenVINO (NPU, GPU, or CPU) and AMD Ryzen AI/Vitis AI (NPU).
You do not need to rebuild Gaze or install a different CLI/GUI. CPU remains the
default; acceleration requires supported hardware, its drivers, and a matching vendor runtime.

## Intel setup

Install your distribution's Intel NPU firmware, kernel support (`intel_vpu`),
and userspace Level Zero driver. On Fedora, these are:

```bash
sudo dnf install intel-npu-driver oneapi-level-zero intel-npu-firmware
```

For other distributions, follow [Intel's Linux NPU driver instructions](https://github.com/intel/linux-npu-driver).
Then install an OpenVINO-enabled ONNX Runtime (1.21 or newer) in a root-owned
location such as `/opt/onnxruntime-openvino`, and [register it](#register-a-runtime)
as the `openvino` runtime.

## AMD setup

[AMD's current Linux guide](https://ryzenai.docs.amd.com/en/latest/linux.html)
documents Ryzen AI 1.8 for STX/KRK platforms (Strix, Strix Halo, Krackan Point)
with Ubuntu 24.04 driver packages. Older Phoenix/Hawk Point NPUs and other Linux
distributions are not included in that documented Linux support target.

Install AMD's XRT/NPU driver packages and Ryzen AI SDK following that guide, in a
root-owned system location such as `/opt/ryzen-ai`. Then [register it](#register-a-runtime)
as the `vitis` runtime, including `/opt/xilinx/xrt/lib` in its library path.

Ryzen AI can [compile FP32 models to BF16](https://ryzenai.docs.amd.com/en/latest/model_quantization.html).
Gaze starts with its existing ONNX models and freezes symbolic input dimensions to
the actual image sizes used by the pipeline. Model compilation/operator support
and numerical accuracy still need validation on the particular NPU and SDK version.

## Register a runtime

`gazed` looks for each vendor runtime in `/usr/lib/gaze/runtimes/<provider>`, where
`<provider>` is `openvino` or `vitis`. That directory holds two entries:

- `libonnxruntime.so`, a symlink to the vendor's ONNX Runtime library
- `library-path`, a colon-separated list of absolute directories holding the SDK's
  other shared libraries

For example, for AMD:

```bash
sudo mkdir -p /usr/lib/gaze/runtimes/vitis
sudo ln -sf /opt/ryzen-ai/onnxruntime/lib/libonnxruntime.so \
    /usr/lib/gaze/runtimes/vitis/libonnxruntime.so
echo /opt/ryzen-ai/onnxruntime/lib:/opt/xilinx/xrt/lib \
    | sudo tee /usr/lib/gaze/runtimes/vitis/library-path
```

Then enable acceleration in `/etc/gaze/config.toml`:

```toml
[inference]
execution_provider = "auto"
device = "npu"
```

and restart the daemon and check each model:

```bash
sudo systemctl restart gazed
gaze doctor --benchmark
```

## Configuration and recovery

You can explicitly select `openvino/npu` or `vitis/npu` instead of automatic selection.
To use an Intel GPU instead, register the `openvino` runtime and set `openvino/gpu`;
`auto` only selects NPUs.
The GUI and `gaze config` expose both providers. Restart the daemon when selecting
a different vendor: one ONNX Runtime library is loaded per process.

Gaze selects automatic mode from `/sys/class/accel` and the bound NPU driver,
rather than CPU branding. `gaze doctor` reports detected devices and missing
registrations. A missing or incompatible vendor runtime, unavailable driver, failed
model compilation, or failed startup inference probe falls back to CPU with a reason
in `gaze doctor --benchmark` and the daemon journal:

```bash
journalctl -u gazed -b
```

Successful provider sessions can contain CPU graph partitions. Timings and provider
labels do not measure operator residency or power consumption. Compare the same
models and settings against a `cpu/cpu` baseline.

Compilation caches live under `/var/cache/gaze/inference`, keyed by model contents,
runtime identity/version, and kernel release. After upgrading a vendor's userspace
driver or SDK, clear that vendor's cache with `sudo rm -rf /var/cache/gaze/inference/<provider>`.
The initial compilation can take longer than subsequent daemon starts.

Keep runtime libraries and their dependencies root-owned, outside `/home` and `/root`
(which `gazed.service` hides), and not writable by other users. Gaze validates the
library's ONNX Runtime API before loading it, and re-executes the daemon with only the
selected vendor's `library-path` before starting its threads.

To return to the default, choose `cpu/cpu` in `gaze config` and restart `gazed`.

For NixOS, use Nix-managed driver/runtime packages and the service environment to
set `ORT_DYLIB_PATH` and `LD_LIBRARY_PATH`, with the same inference settings.
The `/usr/lib/gaze/runtimes` registration is intended for conventional distro packages.

## Hardware validation

For each vendor, test both model qualities, RGB/IR recognizers, MiniFASNet liveness,
and the eye-state model if enabled. Confirm startup/warmup behavior, cold versus cached
startup times, and per-model mean/p95 latency. Compare recognition and liveness scores
against CPU on representative genuine and spoof samples; include embeddings enrolled
before acceleration was enabled. Keep the existing thresholds unless validation
supports a deliberate change.

Exercise absent drivers, a missing SDK dependency, unsupported model operators,
driver/SDK upgrades, and a switch back to CPU. Verify password fallback and recovery
without relaxing the service's sandbox. CI tests configuration, discovery, native API
validation, and inference fallback without physical NPUs; it does not certify model
accuracy or performance on either vendor's hardware.
