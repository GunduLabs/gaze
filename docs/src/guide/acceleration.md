<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Hardware acceleration

Gaze can use Intel OpenVINO (NPU, GPU, or CPU) and AMD Ryzen AI/Vitis AI (NPU)
with its standard daemon; there is no need to rebuild Gaze or replace the CLI or
GUI. By default, Gaze runs inference on the CPU, so you can use it without any
acceleration setup. If you'd like to use supported accelerator hardware, install
the matching drivers and vendor runtime first.

## Intel setup

For Intel NPU acceleration, start by installing your distribution's NPU firmware,
kernel support (`intel_vpu`), and userspace Level Zero driver. On Fedora, install:

```bash
sudo dnf install intel-npu-driver oneapi-level-zero intel-npu-firmware
```

On other distributions, [follow Intel's Linux NPU driver instructions](https://github.com/intel/linux-npu-driver).
Next, install an OpenVINO-enabled ONNX Runtime (1.21 or newer) in a root-owned
location such as `/opt/onnxruntime-openvino`, then [register it](#register-a-runtime)
as the `openvino` runtime.

## AMD setup

[AMD's current Linux guide](https://ryzenai.docs.amd.com/en/latest/linux.html)
documents Ryzen AI 1.8 for STX/KRK platforms (Strix, Strix Halo, Krackan Point)
with Ubuntu 24.04 driver packages. Older Phoenix/Hawk Point NPUs and other Linux
distributions are not included in that documented Linux support target.

To set up AMD acceleration, follow that guide to install AMD's XRT/NPU driver
packages and Ryzen AI SDK in a root-owned system location such as `/opt/ryzen-ai`.
Then [register the runtime](#register-a-runtime) as `vitis`, including
`/opt/xilinx/xrt/lib` in its library path.

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

Once the runtime is registered, enable acceleration in `/etc/gaze/config.toml`:

```toml
[inference]
execution_provider = "auto"
device = "npu"
```

Finally, restart the daemon and check that each model is using the expected provider:

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

For security, keep runtime libraries and their dependencies root-owned, outside `/home` and `/root`
(which `gazed.service` hides), and not writable by other users. Gaze validates the
library's ONNX Runtime API before loading it, and re-executes the daemon with only the
selected vendor's `library-path` before starting its threads.

To switch back to CPU inference, choose `cpu/cpu` in `gaze config` and restart `gazed`.

For NixOS, use Nix-managed driver/runtime packages and the service environment to
set `ORT_DYLIB_PATH` and `LD_LIBRARY_PATH`, with the same inference settings.
The `/usr/lib/gaze/runtimes` registration is intended for conventional distro packages.

## Hardware validation

For each vendor, test both model qualities, the RGB and IR recognizers,
MiniFASNet liveness, and the eye-state model if enabled. Check startup and
warm-up behavior, cold and cached startup times, and each model's mean and p95
latency. Compare recognition and liveness scores with CPU results on representative
genuine and spoof samples, including profiles enrolled before acceleration was
enabled. Keep the existing thresholds unless validation supports changing them.

Exercise absent drivers, a missing SDK dependency, unsupported model operators,
driver/SDK upgrades, and a switch back to CPU. Verify password fallback and recovery
without relaxing the service's sandbox. CI tests configuration, discovery, native API
validation, and inference fallback without physical NPUs; it does not certify model
accuracy or performance on either vendor's hardware.
