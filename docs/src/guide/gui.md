# GUI Guide

`gaze-gui` is the easiest way to enroll faces and check auth health.

Launch it:

```bash
gaze-gui
```

- **Enroll a new face profile**: Initiates a guided camera capture. If both RGB and IR cameras are configured, it captures from both.
- **View enrolled profiles**: The main window lists enrolled faces with green/red `RGB` and `IR` badges indicating which capture types are active, along with the total template capture count.
- **Refine profiles**: Tap the edit/refine icon on a profile to capture additional samples or add a missing spectrum (e.g. adding IR captures to an existing RGB-only face profile after configuring an IR camera).
- **Test authentication**: Check Gaze's recognition with immediate pass/fail visual feedback.
- **Remove profiles**: Delete specific face profiles.
- **Configure daemon settings**: Change security levels, cameras, liveness settings, and hybrid policies.

## Configuration dialog

Open the config dialog from the header-bar settings button.

From there you can edit:

- Security level (`low`, `medium`, `high`, `maximum`, or custom models/threshold)
- RGB camera source, IR camera device, and IR emitter
- Dark-frame rejection cutoff
- Maximum enrollment templates per face
- Liveness anti-spoofing (enable, threshold, max frames)
- Auth behavior (abort if SSH, abort if lid closed, require confirmation)

## Common tasks

1. Enroll a profile named `default`.
2. Run test authentication several times in normal room light.
3. Add another profile if your appearance varies often (for example, glasses).

## When to use GUI vs CLI

- Use GUI for enrollment and quick pass/fail checks.
- Use CLI (`gaze auth --verbose`) when you want detailed authentication metrics and diagnostics.

## If the GUI cannot authenticate

Check daemon status:

```bash
systemctl status gazed
```

If stopped:

```bash
sudo systemctl enable --now gazed
```

Then retry from GUI.
