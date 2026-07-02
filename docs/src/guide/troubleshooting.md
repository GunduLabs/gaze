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

If the TPM was reset/cleared after you enrolled, the previously sealed key can no longer be unsealed. Delete the stale key directory and re-enroll your faces:

```bash
sudo rm -rf /var/lib/gaze/tpm
sudo systemctl restart gazed
```

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

Direct `/dev/video*` paths are not supported.

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

## 4. Lock screen does not trigger face auth

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

## 5. PAM auth flow seems broken

Reinstall packages (recommended):

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This reapplies package-managed PAM integration.

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

If you see errors like repository connection failures, metadata hash mismatches, or repository GPG signature failures when running `apt update` or `dnf makecache`, reinstall the current package source configuration from the [Installation guide](/guide/installation).

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

## 11. Collect useful logs before asking for help

```bash
gaze doctor
systemctl status gazed
journalctl -u gazed -n 300 --no-pager
gaze auth --verbose
```

Include the complete `gaze doctor` output, distro version, and desktop environment (GNOME/KDE/etc.) when reporting issues.
