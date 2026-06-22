# Contributing

Thanks for helping improve Gaze. This guide covers how to propose changes, what to test, and what to avoid when working on authentication, PAM, packaging, and docs.

For source builds and component-specific setup, start with the [development guide](/guide/development).

## Ways to contribute

- Report reproducible bugs with logs, distro version, desktop environment, and the command that failed.
- Improve docs when behavior is unclear, missing, or distro-specific.
- Add focused tests for pure logic, edge cases, and regressions.
- Fix packaging, install, or uninstall issues for supported distributions.
- Improve camera, DBus, CLI, GUI, PAM, or GNOME extension behavior.

## Before you start

- Check existing issues and pull requests so work is not duplicated.
- Open an issue or discussion first for large behavior changes, new config keys, packaging policy changes, or authentication flow changes.
- Keep changes small and reviewable. Prefer one bug fix or feature per pull request.
- Do not commit downloaded ML models, face embeddings, local config, package artifacts, or secrets.

## Local setup

Clone the repo, install dependencies from the [development guide](/guide/development), then run:

```bash
just setup-hooks
just --list
```

The hook setup is local to your clone. CI still runs the required checks for pushes and pull requests.

## Workflow

1. Create a branch with a short descriptive name.
2. Make the smallest correct change.
3. Add or update tests when behavior changes.
4. Update docs when user-visible behavior, install steps, config, CLI output, or packaging behavior changes.
5. Run the relevant checks locally.
6. Open a pull request with a clear summary, testing notes, and any manual verification steps.

## Required checks

Run these before opening a pull request:

```bash
just fmt-check
just lint
just test
just audit
```

If you changed the `Justfile`, also run:

```bash
just --fmt --check
```

If you changed packaging files, scripts, systemd units, DBus policy, PAM integration, or GNOME extension packaging, build at least the affected package format:

```bash
just package <deb | rpm | archlinux>
```

## Tests

Prefer tests that run in CI without hardware or system services.

Good test targets:

- Config parsing, defaults, and DBus map conversion.
- User database validation, persistence, matching, and error paths.
- Model helper logic that does not download files.
- Alignment, preprocessing, and other pure math or image transforms.
- CLI parsing and display helper behavior.

Avoid CI tests that require:

- A physical camera.
- A running system DBus `gazed` service.
- PAM installed into system auth files.
- A graphical session.
- Network access to download model packs.

Use manual test notes for those areas instead.

## Manual testing

For daemon changes, stop the installed service and run the local build in the foreground:

```bash
sudo systemctl stop gazed
just build-rust
sudo RUST_LOG=debug ./target/release/gazed
```

Then exercise clients against the daemon that owns the system bus:

```bash
./target/release/gaze list-faces
./target/release/gaze auth --verbose
./target/release/gaze-gui
```

Restart the installed service when finished:

```bash
sudo systemctl start gazed
```

## PAM safety

PAM changes can lock you out of authentication flows.

- Keep a second terminal open with an active root shell before editing PAM files.
- Test with a non-critical PAM service first, not `sudo`, `system-auth`, or your graphical login.
- Be careful with unsafe FFI in `pam-gaze` and `pam-gaze-grosshack`.
- Include exact manual test steps in the pull request.

## Docs style

- Write for users first. Put the command they should run before long explanations.
- Mention distro differences when paths or packages differ.
- Use fenced code blocks for commands and config snippets.
- Keep warnings explicit for security, PAM, GDM, and lockout risks.
- Link to nearby pages instead of repeating long setup instructions.
- Do not edit generated files under `docs/.vitepress/dist`; edit Markdown or theme files and rebuild docs.

To preview docs locally:

```bash
bun run docs:dev
```

To verify the docs build:

```bash
bun run docs:build
```

## Bug reports

Useful bug reports include:

- Gaze version and install method.
- Distribution and desktop environment.
- Camera source from `gaze config` or `/etc/gaze/config.toml`.
- The exact command or flow that failed.
- Relevant logs from `journalctl -u gazed -n 300 --no-pager`.
- Whether the issue affects CLI auth, GUI enrollment, PAM, GNOME lock screen, or GDM login.

Remove private data before sharing logs.

## Pull request notes

In the pull request description, include:

- What changed.
- Why it changed.
- Tests run.
- Manual verification, if hardware, DBus, PAM, GNOME, or packaging behavior was involved.
- Follow-up work or known limitations.
