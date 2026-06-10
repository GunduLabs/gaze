# GUI Guide

`gaze-gui` is the easiest way to enroll faces and check auth health.

Launch it:

```bash
gaze-gui
```

## What you can do in the GUI

- Enroll a new face profile
- Test authentication with immediate pass/fail feedback
- View and remove enrolled profiles
- Configure daemon settings from the header-bar config button
- Toggle GDM face auth 

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
- Use CLI (`gaze auth --verbose`) when you want similarity scores and diagnostics.

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
