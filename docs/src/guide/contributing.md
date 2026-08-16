<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Contributing

Thanks for helping improve Gaze. This guide covers how to propose changes, what to test, and what to avoid when working on authentication, PAM, packaging, and docs.

For source builds and component-specific setup, start with the [development guide](/guide/development).

## Ways to contribute

- Report reproducible bugs with logs, distro version, desktop environment, and the command that failed. See [opening an issue](#opening-an-issue).
- Improve docs when behavior is unclear, missing, or distro-specific.
- Add focused tests for pure logic, edge cases, and regressions.
- Fix packaging, install, or uninstall issues for supported distributions.
- Improve camera, DBus, CLI, GUI, PAM, or GNOME extension behavior.

## Before you start

- Check existing issues and pull requests so work is not duplicated.
- Open an issue or discussion first for large behavior changes, new config keys, packaging policy changes, or authentication flow changes.
- Keep changes small and reviewable. Prefer one bug fix or feature per pull request.
- Do not commit downloaded ML models, face embeddings, local config, package artifacts, or secrets.
- If you used an AI tool, read [AI-assisted contributions](#ai-assisted-contributions) before you open the pull request.

## Licensing

Gaze is licensed under the [GNU General Public License, version 3 or later](https://github.com/GunduLabs/gaze/blob/main/LICENSE) (`GPL-3.0-or-later`). By opening a pull request you agree that your contribution is licensed under the same terms.

- Start every new source file with the SPDX header used throughout the tree:

  ```rust
  // SPDX-FileCopyrightText: 2026 Gundu Labs
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```

  Shell scripts use `#` comments and place the header directly below the shebang.

- Do not copy code into Gaze from projects under a license that is incompatible with GPLv3, and do not add dependencies that cannot be distributed alongside GPLv3 code. `just audit` does not check licenses, so check the license of any new dependency yourself.
- New crate manifests inherit the license from the workspace with `license.workspace = true`; new packaging files must declare `GPL-3.0-or-later`.

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
6. Open a pull request and fill in the template. See [opening a pull request](#opening-a-pull-request).

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

## Opening an issue

[Open a new issue](https://github.com/GunduLabs/gaze/issues/new/choose) and pick the template that fits:

| Template | Use it for |
| --- | --- |
| Bug report | Face auth, the camera, the daemon, the GUI, or a desktop integration behaving incorrectly. |
| Installation & packaging | Anything that fails before Gaze runs: the installer, a package manager, a repository or GPG key, a missing shared library, a conflicting package, a broken upgrade. |
| Feature request | A new capability, config key, desktop integration, or distribution. |
| Documentation | Docs that are wrong, missing, outdated, or unclear. |

The forms ask for what we would otherwise have to come back and ask for, which is the slowest part of resolving a report. Every field exists because a past issue stalled without it.

### What a bug report needs

The bug form collects all of this, but if you are adding to an existing issue, these are the things worth including:

- Gaze version (`gaze --version`) and install method. Mixing repositories, or switching between the AUR package and the official one, causes real breakage.
- Distribution and version, desktop environment and version, session type (Wayland or X11), and the display manager if a greeter or lock screen is involved. Several bugs are specific to one GNOME Shell or Plasma release.
- The complete output of `gaze doctor`, run as your desktop user from inside a graphical session rather than over SSH or under `sudo`, so it can see that user's PipeWire session and desktop integration. Include the checks that pass, not only the ones that fail.
- Daemon logs covering the failure, from `journalctl -u gazed -b --no-pager | tail -300`. The lines before the failure usually explain it.
- Which surfaces are affected, and which ones work. A face match that succeeds under `gaze auth` and fails at a greeter points somewhere very different from one that fails everywhere.
- For camera, IR, or recognition problems: `gaze config --show`, `gaze auth --verbose`, and the camera's vendor and model IDs from `udevadm info -q property -n /dev/video0`. Those IDs are what tell us whether your hardware needs a dedicated IR-emitter profile.
- For KDE: `gaze-kde-pam status` and your PAM stacks. On Fedora the vendor copy lives under `/usr/lib/pam.d` and only appears in `/etc/pam.d` once something has customized it, so include both paths.
- Whether you have run `gaze uninstall` at any point. It deliberately removes `/etc/gaze/config.toml`, so a reinstall afterwards gives you a default config, which looks exactly like a bug that lost your settings.
- Whether this ever worked, and the last version that did.

Remove private data before sharing logs. Logs can carry usernames, hostnames, and device serial numbers. Face templates and embeddings are never written to logs, so those are not a concern.

Report vulnerabilities in authentication, PAM, or template storage through a [private security advisory](https://github.com/GunduLabs/gaze/security/advisories/new) rather than a public issue.

## Opening a pull request

The [pull request template](https://github.com/GunduLabs/gaze/blob/main/.github/pull_request_template.md) loads automatically. Delete the sections that do not apply, but keep **Verification** and **AI disclosure**, which are required on every pull request.

Include:

- What changed and why, led by the behavior difference a user would notice.
- The root cause, for bug fixes. When the fix is one line, this is the valuable half of the pull request.
- The output of the [required checks](#required-checks), pasted rather than merely asserted.
- Tests added or updated, or a note that the behavior is not testable in CI without hardware or system services.
- Manual verification steps precise enough for someone else to repeat, plus what you ran them on: distribution, desktop, session type, display manager, camera model.
- Exact manual test steps for PAM changes, along with the [PAM safety](#pam-safety) precautions you took.
- Confirmation that new files carry the SPDX header and that no license-incompatible code was copied in.
- Follow-up work and known limitations. Saying that you could not test the GDM path because you have no GNOME machine is more useful than silence, and it does not count against the pull request.

## AI-assisted contributions

AI-assisted contributions are welcome and are reviewed on the same terms as any other. Using a model is not disqualifying. Not disclosing it is.

Disclosure is required because it changes what a reviewer has to check. A human who misreads a PAM handle lifetime leaves traces in the surrounding reasoning that a reviewer can follow. A model that fabricates one produces a diff that reads correct all the way down, with confident comments explaining an invariant that was never true. Those need different kinds of attention, and in authentication code the difference matters.

The pull request template asks you to select one of:

- **None.** Written by hand.
- **Assisted.** AI helped with parts of it, such as completion, boilerplate, docs prose, test scaffolding, or naming. You directed the design and read every line.
- **Generated.** AI produced most of the diff from your prompting. You reviewed all of it and understand it.
- **Agentic.** An agent produced it largely on its own, with limited supervision.

Name the tools you used, and say where the model helped and where you had to correct it. That last part is the most useful sentence in the section. Knowing that a model got the happy path right but invented the error handling tells a reviewer exactly where to look first.

Whichever option applies, these hold:

- **Never report a check or a test you did not run.** Every result in the pull request must come from actually running it on a real machine. Not predicted, not inferred from reading the code, and not reported to you by a tool. "Syntax-verified" and "tests in progress, will update" are not test results. A fabricated pass is worse than an admitted gap, because it spends reviewer trust that the next contributor needs.
- **You are the author.** You should be able to explain why every change in the diff is there and defend it in review. If you cannot, the pull request is not ready, no matter how good the code looks.
- **Respond to review yourself.** Feedback forwarded to a tool unread produces plausible replies to questions nobody asked.
- **Respect licensing.** Do not paste proprietary, confidential, or license-incompatible code into an AI tool to produce a contribution, and make sure the output does not reproduce such code. The [licensing rules](#licensing) apply to generated code exactly as they do to written code.

Unsolicited agent-generated pull requests opened against this repository without a human who has read the diff will be closed. Volume is not a contribution.
