<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Troubleshooting

If Gaze is installed but not authenticating reliably, use this page as a quick diagnostic checklist.

Start from a local graphical session:

```bash
gaze doctor
```

This checks the service, config, DBus, PipeWire camera visibility, enrollments, PAM setup, desktop integration, and TPM requirements without capturing camera frames or changing the system. Follow the suggested fix printed below each warning or error. A result with errors exits with status `1`, which also makes the command suitable for support scripts.

## 1. Daemon is not running

Check the daemon:

```bash
systemctl status gazed
```

If the output says `active (running)`, this part is fine.

Fix:

```bash
sudo systemctl enable --now gazed
```

If it still fails:

```bash
journalctl -u gazed -n 200 --no-pager
```

That command shows the most recent daemon log messages.

### Daemon exits when template encryption is enabled

If `gazed` refuses to start and the logs show a message like *"template encryption is enabled ([storage] encrypt_templates) but no usable TPM is available"*, the daemon is failing closed on purpose: it will not store biometric data unencrypted once you have asked for encryption.

```bash
journalctl -u gazed -n 50 --no-pager
```

Fix it one of two ways:

- Enable the TPM. Confirm a TPM 2.0 device exists (`ls /dev/tpmrm0`) and is turned on in your firmware/BIOS, then restart: `sudo systemctl restart gazed`.
- Turn the feature off. Set `encrypt_templates = false` under `[storage]` in `/etc/gaze/config.toml` and restart.

If the logs instead show *"Failed to open specified TCTI device file /dev/tpmrm0: Permission denied"*, the device exists but the daemon may not open it. Distributions restrict the TPM nodes to the `tss` user and group, so `gazed` needs `CAP_DAC_OVERRIDE` (which the packaged unit grants) or membership in that group. Check the node and the unit:

```bash
ls -l /dev/tpmrm0 /dev/tpm0
systemctl show gazed -p CapabilityBoundingSet -p SupplementaryGroups
```

If a local override or drop-in tightened `CapabilityBoundingSet`, add `SupplementaryGroups=tss` instead:

```bash
sudo systemctl edit gazed   # [Service] / SupplementaryGroups=tss
sudo systemctl restart gazed
```

If the TPM was reset/cleared after you enrolled, the previously sealed key can no longer be unsealed. Delete the stale key directory and re-enroll your faces:

```bash
sudo rm -rf /var/lib/gaze/tpm
sudo systemctl restart gazed
```

### Daemon fails with "error while loading shared libraries: libopencv_*" (Arch Linux)

On Arch Linux and Arch-compatible distributions, a system update that bumps
OpenCV to a new minor version (for example 4.13 to 5.0) removes the library
version `gazed` was built against, and the daemon exits immediately with
status 127. Update to a `gaze-bin` release built against the new OpenCV:

```bash
yay -Syu gaze-bin
```

Newer packages declare a version-bounded `opencv` dependency, so pacman
refuses the OpenCV upgrade up front instead of breaking the installed daemon.
If pacman reports that upgrading `opencv` would break the dependency and no
updated `gaze-bin` exists yet, wait for the rebuilt release before upgrading,
or build and install Gaze from source against the new OpenCV.

### Gaze disappears after an AUR helper upgrade (Arch Linux)

If face auth stops working across every surface at once (sudo, polkit, the lock
screen, greetd) and `systemctl status gazed` reports that the unit does not
exist, check whether an AUR helper replaced Gaze with an unrelated package:

```bash
pacman -Qi gaze 2>/dev/null | head -n 3
ls /usr/lib/security/pam_gaze.so
```

An unrelated AUR package is also named `gaze` (a file watcher, currently at a
much higher version number). If Gaze was installed under the plain names `gaze`
and `gaze-gui`, for example by running `pacman -U` on the `.pkg.tar.zst` files
attached to a GitHub release, pacman records them as foreign packages, and
`yay -Syu` or `paru -Syu` treats the unrelated AUR `gaze` as a newer version and
installs it over ours. That removes `gazed`, both PAM modules, and the systemd
unit, and turns `/etc/gaze/config.toml` into a `.pacsave` file.

The failure is silent: the `sufficient` PAM lines fail to load the missing
module, logging `PAM adding faulty module: pam_gaze.so`, and every surface falls
back to fingerprint or password.

Recover by removing the wrong package and reinstalling the AUR packages.
Enrolled templates live in `/var/lib/gaze` and survive this:

```bash
sudo pacman -Rns gaze gaze-gui
yay -S --needed gaze-bin gaze-gui-bin
sudo systemctl enable --now gazed
```

If `/etc/gaze/config.toml.pacsave` exists, restore your settings from it. The
AUR packages install under the names `gaze-bin` and `gaze-gui-bin`, which do not
collide, so this cannot happen again once you are on them.

### Daemon fails with "error while loading shared libraries: libopencv_*" (Debian/Ubuntu)

```
gazed: error while loading shared libraries: libopencv_core.so.406:
cannot open shared object file: No such file or directory
```

Each Debian/Ubuntu release ships a different OpenCV soversion (24.04 has 4.6,
26.04 has 4.10), so this means the installed package was built for another
release. Check which one you have and which suite apt is pointed at:

```bash
dpkg-query -W -f='${Version}\n' gaze
grep gundulabs /etc/apt/sources.list.d/gundulabs.list
```

The version suffix (`1~ubuntu24.04`, `1~ubuntu26.04`, `1~debian13`, `1~debian14`) has to match
your release. Point apt at the suite for your release and reinstall:

```bash
suite="$(. /etc/os-release && echo "${VERSION_CODENAME:-$UBUNTU_CODENAME}")"
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb $suite main" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install --reinstall gaze gaze-gui
sudo systemctl restart gazed
```

Do not symlink the newer OpenCV libraries to the missing soversions. The soname
changes because the C++ ABI changed, so the daemon may start but can crash or
corrupt data later.

## 2. Camera is not detected

Use the primary GStreamer camera source first:

```toml
[cameras]
rgb = "primary"
```

If you need a specific camera, run `gaze config` and select one of the detected PipeWire cameras, or set a GStreamer source manually:

```toml
[cameras]
rgb = "pipewiresrc target-object=<pipewire-target>"
```

You can also point `rgb` at a camera directly with a `/dev/video*` node or a `usb:VVVV:PPPP` id, which use `v4l2src` and need no PipeWire session.

When `rgb = "primary"` and the daemon cannot reach a PipeWire session (common when authenticating from the GDM login screen, where `gazed` runs without the user's session), Gaze automatically falls back to the first color `/dev/video*` node so face auth still works. Set `rgb` to a specific `/dev/video*` node or `usb:VVVV:PPPP` id if the fallback picks the wrong camera.

Then restart daemon:

```bash
sudo systemctl restart gazed
```

## 3. Enrollment works, auth fails often

Try this sequence:

1. Keep `level = "medium"` in config.
2. Improve sample coverage:

```bash
gaze refine-face default
```

3. Test scores:

```bash
gaze auth --verbose
```

4. Add a second profile for a common variation:

```bash
gaze add-face glasses
```

If similarity scores dropped right after upgrading Gaze on a machine with a
widescreen (16:9) camera, re-enroll your faces once: older releases stretched
widescreen frames to 4:3, so templates enrolled before the fix will not match
undistorted frames as well as freshly enrolled ones.

### `gaze auth --verbose` shows a passing score but still fails

A face that clears the similarity threshold is only half the decision: the
anti-spoof check has to pass as well. When it does not, the run ends with

```
Face matched, but the liveness check did not pass.
```

and `journalctl -u gazed -b` records `VerifyStart: frame budget spent` with
`matched=true`. Move slightly during the scan, raise the light on your face, or lower
`liveness.threshold` in `/etc/gaze/config.toml` if your camera consistently
scores below it:

```toml
[liveness]
threshold = 0.7
```

Setting `enabled = false` in the same section turns the anti-spoof model off
entirely, at the cost of accepting a photograph.

### Auth aborts with "IR camera stream stopped unexpectedly"

Some single-function Windows Hello webcams can't stream their RGB and IR sensors
at the same time. The Logitech BRIO 4K (`046d:085e`, not the newer Brio
300/500/100, which use different product IDs) is a known example. Enrollment
works fine, but capturing RGB and IR in parallel drops the IR substream and the
attempt falls back to your password.

Gaze captures one camera at a time by default, so first check whether
`cameras.parallel_capture` has been set:

```bash
gaze config --show | grep parallel_capture
```

If it is `always`, set it to `auto` (which serializes cameras whose sensors share
one hardware function) or `never`. If you see this on `never`, the two sensors
cannot coexist even sequentially. Configure the IR camera on its own and leave
`rgb` empty so Gaze runs IR-only:

```toml
[cameras]
rgb = ""
ir = "/dev/video2"
emitter_enabled = true
```

## 4. Lock screen does not trigger face auth

### GNOME

Enable or re-enable the extension from your GNOME session:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

If `gnome-extensions enable` reports `Extension "gaze@gundulabs.com" does not exist`, GNOME Shell has not picked up the newly installed extension yet. Reboot, then re-run the command. On Wayland this is the only way; Shell does not rescan extensions in a running session. The one-line installer works around this by writing the equivalent dconf keys directly, which take effect on the next login without needing `gnome-extensions enable` to succeed.

For GDM login, if the face-auth text appears but the camera light never turns on, check the daemon logs for camera/PipeWire errors:

```bash
journalctl -u gazed -b
```

Older Gaze builds could try to use the selected user's PipeWire runtime before that user session existed. Update Gaze if you see this behavior.

#### GDM never scans on Fedora or SELinux-enabled systems

If face auth works in your desktop session (`sudo`, the lock screen, `gaze auth`)
but the GDM login screen never opens the camera, SELinux may be denying the
greeter the camera device. Gaze ships a policy module for this and the
extension package loads it on install, but that step cannot fail the package
transaction, so a rejected policy leaves no trace beyond the greeter silently
not scanning. openSUSE normally uses AppArmor; use this check there only if you
have explicitly enabled SELinux.

`gaze doctor` reports this as **GDM camera SELinux policy**. To check by hand:

```bash
sudo semodule -l | grep gaze-gdm-camera
sudo ausearch -m avc -ts today | grep xdm_t
```

If the module is not listed, load it and reboot:

```bash
sudo semodule -i /usr/share/gaze/gaze-gdm-camera.pp
sudo reboot
```

### KDE Plasma

Check the "KDE lock screen" line in `gaze doctor`, then:

```bash
sudo gaze-kde-pam enable
gaze-kde-pam status
```

The lock screen only starts face auth on its own when `/etc/pam.d/kde-fingerprint`
runs `pam_gaze.so`. Without it, KScreenLocker has no biometric slot to start and
face auth waits until you submit the password field.

If `gaze doctor` warns that the slot runs `pam_gaze_grosshack.so`, replace it with
the plain module: the simultaneous one waits for a password prompt the greeter can
never answer, which wedges the slot for the rest of the lock.

At the **login greeter** (Plasma Login Manager or SDDM), face auth is opt-in and,
unless the greeter ships an up-front biometric service, starts only when the login
form is submitted: press Enter with the password field empty, exactly as you would
for a fingerprint reader there. See the [KDE Plasma guide](/guide/kde).

## 5. PAM auth flow seems broken

Reinstall packages (recommended):

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This reapplies package-managed PAM integration.

### Bitwarden browser extension never starts face authentication

Bitwarden browser unlock reaches Gaze indirectly: the extension talks to the
Bitwarden desktop app through native messaging, and the desktop app requests a
Polkit authorization. First verify that the `polkit-1` PAM path starts Gaze:

```bash
gaze doctor
pkexec /usr/bin/true
```

If that works but clicking **Unlock with biometrics** in the extension opens no
Polkit prompt, troubleshoot Bitwarden's desktop app and native-messaging
connection rather than adding another PAM rule. If the Polkit test does not use
Gaze, fix the distribution-specific `polkit-1` setup. See
[Browser extensions through Polkit](/guide/pam#browser-extensions-through-polkit-bitwarden)
for the complete setup and diagnostic split.

## 6. First run is slow

This is normal when models are downloaded initially.

After first successful run, subsequent auth attempts should be faster.

## 7. Verify installed version and binaries

```bash
gaze --version
which gaze
which gaze-gui
```

What these do:

- `gaze --version`: confirms the CLI is installed
- `which gaze`: shows where the CLI binary is located
- `which gaze-gui`: shows where the GUI binary is located

## 8. Package repository is not loading or signatures mismatch

If you see errors like repository connection failures, metadata hash mismatches, or repository GPG signature failures when running `apt update`, `dnf makecache`, or `zypper refresh`, reinstall the current package source configuration from the [Installation guide](/guide/installation). On openSUSE Tumbleweed, confirm that the repository URL is the Tumbleweed repository and that its architecture is `x86_64`; Fedora repositories are not compatible with Tumbleweed's rolling-library ABI.

## 9. PAM module fails to load on Ubuntu 26.04+

If `journalctl` shows lines like:

```
PAM unable to dlopen(pam_gaze.so): /usr/lib/security/pam_gaze.so: cannot open shared object file
PAM adding faulty module: pam_gaze.so
```

your installed package predates the fix for Ubuntu 26.04's PAM module search path. Update to the latest packages with the one-line installer:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

## 10. Crash on launch (SIGSEGV) on older CPUs

On CPUs without AVX2 (roughly pre-2013), older builds of `gaze` and `gaze-gui` crashed immediately with a segmentation fault because the ONNX Runtime they statically linked requires AVX2. Current packages no longer link ONNX Runtime into the client binaries, so update to the latest packages if you see this. The `gazed` daemon itself still requires a CPU with AVX2.

## 11. Daemon dumps core with "Failed to initialize ORT API"

```
The requested API version [27] is not available, only API versions [1, 26] are supported in this build. Current ORT Version is: 1.26.0
thread 'main' panicked at ort-2.0.0-rc.13/src/lib.rs: Failed to initialize ORT API
```

The ONNX Runtime `gazed` links against is older than the ONNX Runtime API the
build asks for. It only affects builds that link a system ONNX Runtime
(`ORT_STRATEGY=system`), such as the Nix package, the Flatpak, and RPM source
builds; the released `.deb`, `.rpm`, and Arch packages bundle their own runtime.

Gaze requires ONNX Runtime 1.21 or newer. Current builds report the mismatch and
exit with an error instead of aborting:

```
the ONNX Runtime library loaded at startup is version 1.20.0, which is older than the 1.21.x this build of Gaze requires
```

Update `gazed` to a current release, or build it against an ONNX Runtime that is
at least 1.21.

## 12. Collect useful logs before asking for help

```bash
gaze doctor
systemctl status gazed
journalctl -u gazed -n 300 --no-pager
gaze auth --verbose
```

Include the complete `gaze doctor` output, distro version, and desktop environment (GNOME/KDE/etc.) when reporting issues. On KDE, add `gaze-kde-pam status` and the contents of `/etc/pam.d/kde-fingerprint`.
