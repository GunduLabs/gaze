<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Troubleshooting

If Gaze is installed but not authenticating reliably, use this page as a quick diagnostic checklist.

Start from a local graphical session:

```bash
gaze doctor
```

This checks the service, config, DBus, PipeWire camera visibility, enrollments, PAM setup, desktop integration, and TPM requirements without capturing camera frames or changing the system. Follow the suggested fix printed below each warning or error. A result with errors exits with status `1`, which also makes the command suitable for support scripts.

Each line is marked with what it found:

| Mark | Meaning |
|---|---|
| `✓` | Working. |
| `○` | A working feature you have switched off, such as GDM login face auth or template encryption. Not a problem. The dim line underneath is the recipe that switches it on. |
| `!` | Warning: Gaze still works, but something is degraded or not wired up. |
| `✗` | Error: this part will not work until you fix it. `gaze doctor` exits `1`. |

If an attempt is ending sooner than you expect, or you want to know which deadline a message
came from, [One authentication attempt](/guide/how-it-works#one-authentication-attempt) walks
through the whole flow, and [Timeouts and budgets](/guide/how-it-works#timeouts-and-budgets)
lists every deadline with the log line it produces.

## 1. Daemon is not running

Check the daemon:

```bash
systemctl status gazed
```

If the output says `active (running)`, this part is fine.

To start and enable the service, run:

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

The version suffix (`1~ubuntu24.04`, `1~ubuntu25.10`, `1~ubuntu26.04`,
`1~ubuntu26.10`, `1~debian13`, `1~debian14`) has to match your release. Point
apt at the suite for your release and reinstall:

```bash
suite="$(. /etc/os-release && echo "${VERSION_CODENAME:-$UBUNTU_CODENAME}")"
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb $suite main" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install --reinstall gaze gaze-gui
sudo systemctl restart gazed
```

Avoid symlinking newer OpenCV libraries to the missing sonames. The soname
changed because the C++ ABI changed, so the daemon might start but later crash
or corrupt data.

## 2. Camera is not detected

First check that the GStreamer plugins Gaze loads at runtime are installed. They
are loaded dynamically, so a missing one shows up as `no element "jpegdec"`,
`no element "v4l2src"`, or `no element "pipewiresrc"` rather than a link error:

```bash
gaze doctor
```

If the `GStreamer plugins` check fails, install the base, good, and PipeWire
plugin packages for your distribution:

::: code-group

```bash [Debian/Ubuntu]
sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire
```

```bash [Fedora/RHEL]
sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer
```

```bash [openSUSE Tumbleweed]
sudo zypper install gstreamer-plugins-base gstreamer-plugins-good gstreamer-plugin-pipewire
```

```bash [Arch Linux / Manjaro]
sudo pacman -S gst-plugins-base gst-plugins-good gst-plugin-pipewire
```

:::

Then restart the daemon with `sudo systemctl restart gazed`.

As a first step, try the primary GStreamer camera source:

```toml
[cameras]
rgb = "primary"
```

If you need a specific camera, run `gaze config` and select one of the detected PipeWire cameras, or set a GStreamer source manually:

```toml
[cameras]
rgb = "pipewiresrc target-object=<pipewire-target>"
```

You can also point `rgb` at a camera directly with a `/dev/video*` node or a `usb:VVVV:PPPP` id, which name the kernel node outright.

For authentication, the daemon always captures the kernel `/dev/video*` node directly and never needs a PipeWire session, not even on the GDM login screen. `rgb = "primary"` means the first color node; set `rgb` to a specific `/dev/video*` node or `usb:VVVV:PPPP` id if that picks the wrong camera. A `pipewiresrc target-object=` value is resolved to the V4L2 node behind that same camera.

### Intel IPU6/IPU7 MIPI cameras (`does not support ... colorimetry`)

Many recent Intel laptops (Meteor Lake, Lunar Lake, and others) use a MIPI camera behind an Intel IPU. Its `/dev/video*` nodes are raw capture nodes that only stream after libcamera configures the camera, so opening one directly fails with an error like `Device '/dev/video0' does not support 2:0:0:0 colorimetry`.

With `rgb = "primary"`, Gaze falls back to libcamera when the color node it picked can only be driven through libcamera, or when no color node is visible. The daemon runs libcamera itself and never uses the user's PipeWire session. Install the libcamera GStreamer plugin and restart the daemon:

::: code-group

```bash [Debian/Ubuntu]
sudo apt install gstreamer1.0-libcamera
```

```bash [Fedora/RHEL]
sudo dnf install libcamera-gstreamer
```

```bash [openSUSE Tumbleweed]
sudo zypper install gstreamer-plugins-libcamera
```

```bash [Arch Linux / Manjaro]
sudo pacman -S libcamera gst-plugin-libcamera
```

:::

If the daemon log shows `Failed to fork: Function not implemented` from libcamera, your libcamera build ships IPA modules whose signatures do not verify. libcamera then tries to run them in a separate sandboxed process, which the daemon's hardening blocks. On Fedora 44 this is fixed in `libcamera-0.7.1-2.fc44` and later.

Restart the daemon to apply the camera change:

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

#### GDM shows no face auth at all, and its journal says nothing

Different from the SELinux case below: there the prompt appears and the camera
stays dark, here nothing about face auth appears at the login screen and the
greeter logs nothing, while the lock screen, `sudo` and `gaze auth` all work.

The greeter has GNOME Shell's extension kill switch on, which stops every
extension there regardless of what enables them:

```bash
sudo env DCONF_PROFILE=gdm XDG_CONFIG_HOME=/var/lib/gdm/seat0/config \
  gsettings get org.gnome.shell disable-user-extensions
```

If that prints `true`, the value lives in GDM's own dconf database. That database
is the one layer of the greeter profile that outranks the keyfiles Gaze installs
under `/etc/dconf/db/gdm.d`, so adding an override there cannot help. Clear it at
the source and reboot:

```bash
sudo rm -f /var/lib/gdm/seat0/config/dconf/user
sudo reboot
```

GDM writes the file again with its own defaults. The path is under `/var/lib/gdm3`
on Debian and Ubuntu, and a multi-seat machine has one directory per seat.
`gaze doctor` reports this as **GDM login face auth** and names the exact file.

#### GDM never scans on Fedora or SELinux-enabled systems

If face auth works in your desktop session (`sudo`, the lock screen, `gaze auth`)
but the GDM login screen never opens the camera, SELinux may be denying the
greeter the camera device. Gaze ships a policy module for this and the
extension package loads it on install, but that step cannot fail the package
transaction, so a rejected policy leaves no trace beyond the greeter silently
not scanning. Fedora and openSUSE Tumbleweed installs made since early 2025
enforce SELinux by default; older openSUSE installs use AppArmor, where this
check does not apply.

`gaze doctor` reports this as **GDM camera SELinux policy**, but only under
`sudo`: reading the loaded module list needs root, so a plain `gaze doctor` says
the check could not run rather than whether the module is there. To check by
hand:

```bash
sudo semodule -l | grep gaze-gdm-camera
sudo ausearch -m avc -ts today | grep xdm_t
```

If the module is not listed, load it and reboot:

```bash
sudo semodule -i /usr/share/gaze/gaze-gdm-camera.pp
sudo reboot
```

#### Keyring unlock never happens on SELinux systems

If GDM or lock-screen face login works until you enable `unlock_gnome_keyring`
(or `unlock_kwallet`), and then every attempt ends with "Keyring unlock
unavailable. Enter your password.", the greeter's PAM worker is being denied
what the hand-off needs. It runs confined as `xdm_t`, and the distribution
policy hides its `/etc/shadow` reads behind a `dontaudit` rule, so nothing shows
up in `ausearch`. Gaze ships a second policy module for this and `gaze keyring`
loads it after enrolling a credential; `sudo gaze doctor` reports it as
**Keyring SELinux policy**. The worker also logs the failing step:

```bash
sudo journalctl -b -g 'pam_gaze:'
sudo semodule -l | grep gaze-greeter-keyring
```

If the module is not listed, load it and retry the face login:

```bash
sudo semodule -i /usr/share/gaze/gaze-greeter-keyring.pp
```

### Cinnamon

Check that the extension is enabled from your Cinnamon session:

```bash
gsettings get org.cinnamon enabled-extensions | grep gaze@gundulabs.com
```

If it is missing, enable it from **System Settings → Extensions** or with the
command in the [Cinnamon Extension guide](/guide/cinnamon#enable-the-extension),
then reload Cinnamon (`Alt + F2`, `r`, Enter).

On X11 sessions that use the standalone `cinnamon-screensaver` (Linux Mint 22.x
and similar), the lock screen authenticates through
`/etc/pam.d/cinnamon-screensaver` rather than the extension. Face unlock there
needs `pam_gaze.so` in that service or in the shared auth stack, and it cannot
show a confirmation button, so `require_confirmation_lock_screen = true` is not
enforced on that surface. See
[Lock screen behavior](/guide/cinnamon#lock-screen-behavior).

### KDE Plasma

Check the "KDE lock screen" line in `gaze doctor`, then:

```bash
sudo gaze-kde-pam enable
gaze-kde-pam status
```

The lock screen only starts face auth on its own when `/etc/pam.d/kde-fingerprint`
runs `pam_gaze.so`. Without it, KScreenLocker has no biometric slot to start and
face auth waits until you submit the password field.

If `gaze doctor` warns that the slot runs `pam_gaze.so` in simultaneous mode, replace it with
the plain module without `simultaneous`: the simultaneous mode waits for a password prompt the greeter can
never answer, which wedges the slot for the rest of the lock.

At the **login greeter** (Plasma Login Manager or SDDM), face auth is opt-in and,
unless the greeter ships an up-front biometric service, starts only when the login
form is submitted: press Enter with the password field empty, exactly as you would
for a fingerprint reader there. See the [KDE Plasma guide](/guide/kde).

### Omarchy

Check the "Omarchy lock" line in `gaze doctor`, then from your unlocked desktop:

```bash
gaze-omarchy doctor
gaze-omarchy enable
```

`gaze-omarchy doctor` reports which piece is missing: host compatibility, the
`/etc/pam.d/gaze-omarchy-face` service, enrollment, or the running lock plugin.
After an Omarchy update changes the shell files, face scans stop until
`gaze-omarchy` is updated. See the [Omarchy guide](/guide/omarchy).

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

### Another service stops authenticating after enabling Gaze (dovecot, sshd, cron)

Symptoms are in that service's own log rather than Gaze's. Enabling a Gaze
profile with `pam-auth-update` puts the module in `common-auth`, which nearly
every service includes, so a problem in the module can reach services that have
nothing to do with face authentication. For dovecot, IMAP logins fail and
`mail.log` shows one of two patterns.

The auth worker dies as the module loads:

```
dovecot: auth-worker: Error: OpenBLAS error: Memory allocation still failed after 10 retries, giving up.
dovecot: auth-worker: Error: Unexpected exit - converting to abort
dovecot: auth: Error: auth-worker: Aborted PASSV request for user: Worker process died unexpectedly
dovecot: imap-login: Login aborted: Connection closed (auth service reported temporary failure)
```

Older packages linked OpenCV into `pam_gaze.so`. On Debian and Ubuntu, OpenCV
pulls in OpenBLAS, whose startup code reserves per-thread memory sized for every
CPU core. Services that cap their address space, as dovecot's auth worker does
at 256 MB by default, cannot satisfy that reservation and abort before any Gaze
code runs. Confirm the module no longer links it:

```bash
objdump -p /usr/lib/security/pam_gaze.so | grep NEEDED
```

Nothing beyond libc, libgcc, libm, and the dynamic loader should appear.

Or the auth worker stalls until dovecot gives up on it:

```
dovecot: auth: Error: auth-worker: Aborted PASSV request for user: Lookup timed out
```

Older packages cleared the duress lockout after every successful password login,
including network ones, by starting a thread pool and a D-Bus call inside the
service's own process. Current packages skip network services and remote callers
entirely, before doing any work.

Both are fixed in current packages, so update:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

To restore mail service before updating, disable Gaze in the shared stack with
`sudo pam-auth-update --disable gaze gaze-simultaneous`.

## 6. First run is slow

The first authentication may take a little longer while Gaze downloads the
models. Later attempts should be faster.

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

## 10. Crash on launch (SIGSEGV or SIGILL) on older CPUs

On CPUs without AVX2 (roughly pre-2013 Intel, pre-2015 AMD), older builds of `gaze` and `gaze-gui` crashed immediately with a segmentation fault because the ONNX Runtime they statically linked requires AVX2. Current packages no longer link ONNX Runtime into the client binaries, so update to the latest packages if you see this.

The `gazed` daemon still requires a CPU with AVX2 and cannot be made to work without one. Confirm with:

```bash
grep -qw avx2 /proc/cpuinfo && echo "AVX2 present" || echo "AVX2 missing"
```

Current `gazed` builds detect this at startup and exit with status 78 before reaching ONNX Runtime, and the systemd unit sets `RestartPreventExitStatus=78`, so the service stops in a failed state instead of restarting forever. The journal shows one line:

```
gazed: AVX2 is unavailable; gazed cannot run on this CPU. Use a machine with AVX2 support. The CLI can run here, but the daemon cannot.
```

Older builds (0.3.2 and earlier) instead died with `status=4/ILL` on every start and crash-looped, because `Restart=on-failure` kept rescheduling them:

```
WARNING: This CPU does not support AVX2, which is required by ort's prebuilt ONNX Runtime binaries.
gazed.service: Main process exited, code=dumped, status=4/ILL
gazed.service: Scheduled restart job, restart counter is at 63.
```

If you are stuck on such a build, `sudo systemctl disable --now gazed` stops the loop.

## 11. Daemon dumps core with "Failed to initialize ORT API"

```
The requested API version [27] is not available, only API versions [1, 26] are supported in this build. Current ORT Version is: 1.26.0
thread 'main' panicked at ort-2.0.0-rc.13/src/lib.rs: Failed to initialize ORT API
```

The loaded ONNX Runtime is older than the API Gaze requests. Current builds
validate API 21 before initialization and try the bundled CPU runtime if a
registered vendor runtime is incompatible. Conventional packages bundle their
CPU runtime; Nix uses a Nix-managed library.

Gaze requires ONNX Runtime 1.21 or newer. Update Gaze and its runtime package,
or point `ORT_DYLIB_PATH` at a compatible vendor runtime and provide its
`LD_LIBRARY_PATH`. For Intel or AMD acceleration, register a compatible SDK as
described in [Hardware Acceleration](/guide/acceleration) and restart `gazed`.

## 12. Collect useful logs before asking for help

```bash
gaze doctor
systemctl status gazed
journalctl -u gazed -n 300 --no-pager
gaze auth --verbose
```

Include the complete `gaze doctor` output, distro version, and desktop environment (GNOME/KDE/etc.) when reporting issues. On KDE, add `gaze-kde-pam status` and the contents of `/etc/pam.d/kde-fingerprint`. On Omarchy, add `gaze-omarchy doctor` and `omarchy-shell lock status`.
