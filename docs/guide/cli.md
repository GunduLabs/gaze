# CLI Guide

Use the `gaze` command for enrollment, testing, and managing face profiles.

All commands talk to the running `gazed` daemon over DBus.

## Most common workflow

```bash
gaze add-face default
gaze auth --verbose
gaze refine-face default
gaze list-faces
```

## Authenticate

```bash
gaze auth
```

Useful options:

```bash
gaze auth --verbose   # show score table
gaze auth --perf      # show timing details
```

Result meanings:

- `Authenticated as: ...`: pass
- `Access Denied`: no stored face passed current threshold

## Enroll a new face profile

```bash
gaze add-face <name>
```

Examples:

```bash
gaze add-face default
gaze add-face glasses
```

Use separate profiles when your appearance changes often.

## Improve a profile

```bash
gaze refine-face <name>
```

Use this if recognition is inconsistent in dim light or side angles.

## List, rename, and remove

```bash
gaze list-faces
gaze rename-face <old> <new>
gaze remove-face <name>
```

## Delete all faces for current user

```bash
gaze clear-user
```

This is destructive.

## Manage another user

Most commands support `-u`:

```bash
gaze list-faces -u alice
gaze add-face work -u alice
```

## Troubleshooting commands

```bash
systemctl status gazed
journalctl -u gazed -n 100 --no-pager
gaze auth --verbose
```

If you need help diagnosing failures, see the [troubleshooting guide](/guide/troubleshooting).
