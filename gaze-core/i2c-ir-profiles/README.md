<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# I2C IR emitter profiles

Profiles in this directory describe non-UVC emitter devices controlled through
Linux `i2c-dev`. Each file must provide a reliable match for the capture node
and any bridge source/sensor needed to identify the physical camera, plus
explicit on and off write sequences. The build script validates and compiles
the files into the Gaze binary; profiles are not loaded from user-writable
configuration at runtime.

I2C emitter writes bypass the bound sensor driver's normal ownership. Add a
profile only when the exact device, bus, address, register sequence, and cleanup
behavior have been verified on the named hardware. Do not copy register values
from a different camera model.
