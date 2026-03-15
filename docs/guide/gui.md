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
