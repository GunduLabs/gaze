<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# I2C IR emitter profiles

Profiles in this directory describe non-UVC emitter devices controlled through
Linux `i2c-dev`. The build script validates and compiles them into the Gaze
binary; profiles are never loaded from user-writable configuration at runtime.

I2C emitter writes bypass the bound sensor driver's normal ownership. Add a
profile only when the exact device, address, register, bit mask, and cleanup
behavior have been verified on the named hardware. Do not copy register values
from a different camera model.

File names use lowercase letters, digits, and `-` only, and each profile must
target a distinct `capture_node`.

## Format

```toml
[device]
name           = "Surface Pro 4 OV7251 IR emitter (I2C)"
source         = "where and how the register behavior was verified"
capture_node   = "/dev/video42"         # node Gaze captures from
capture_name   = "Surface IR Camera"    # /sys/class/video4linux/<node>/name
source_marker  = "/run/surface_ir_bridge_dev"  # optional, with source_driver
source_driver  = "ipu3-cio2"
sensor_device  = "i2c-INT347E:00"       # entry under /sys/bus/i2c/devices
sensor_driver  = "ov7251"               # driver that must be bound to it
i2c_address    = 0x60

[emitter]
register       = 0x3005
register_width = 2      # register address bytes, big-endian
mask           = 0x08   # only these bits are changed
on             = 0x08
off            = 0x00
```

## How a profile is matched and applied

- The configured IR node must equal `capture_node` and report `capture_name`.
- If `source_marker` is set, it must contain the path of a `/dev/video*` node
  bound to `source_driver`. This is how a userspace bridge (for example a
  v4l2loopback relay) proves which physical camera feeds `capture_node`.
- `sensor_device` must be bound to `sensor_driver`. The I2C bus is taken from
  the adapter that device sits on, so bus renumbering between boots cannot
  redirect writes to a different adapter.
- Before writing, Gaze confirms the kernel reports `i2c_address` as claimed by
  a driver on that bus, and refuses to write otherwise.
- The register is read, only the `mask` bits are replaced, and the result is
  read back and checked. Once the capture stream delivers its first frame the
  emitter state is applied again, since sensor drivers often reload their
  register tables when streaming starts.
- The `i2c-dev` kernel module must be loaded so `/dev/i2c-*` nodes exist.
