# Troubleshooting

If Gaze is installed but not authenticating reliably, use this page as a quick diagnostic checklist.

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

## 2. Camera is not detected

Check camera devices:

```bash
ls /dev/video*
```

If your camera is not `/dev/video0`, set it in `/etc/gaze/config.toml`:

```toml
[cameras]
rgb = "/dev/video1"
```

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

## 4. Lock screen does not trigger face auth

Enable extension:

```bash
gnome-extensions enable gaze@gundulabs.com
```

On Wayland, log out and back in after extension install or update.

## 5. PAM auth flow seems broken

Reinstall packages (recommended):

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
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

## 8. Collect useful logs before asking for help

```bash
systemctl status gazed
journalctl -u gazed -n 300 --no-pager
gaze auth --verbose
```

Include distro version and desktop environment (GNOME/KDE/etc.) when reporting issues.
