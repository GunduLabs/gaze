<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

<!--
Thanks for contributing to Gaze.

Delete the sections that do not apply. Keep the "Verification" and "AI disclosure"
sections, they are required on every pull request.

Gaze is authentication software. A bug here does not corrupt a document, it locks
someone out of their machine or lets the wrong person in. That is why this template
asks for more than most.
-->

## Summary

<!-- What changed, and why. Lead with the behavior difference a user would notice. -->

## Root cause

<!--
For bug fixes: what was actually wrong, not just what you changed. If the fix is
one line, this section is the valuable half of the pull request.
Delete for features and refactors.
-->

## Related issues

<!-- Closes #123 / Refs #456. Say "none" if this came from nowhere but your own machine. -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor with no behavior change
- [ ] Performance
- [ ] Documentation
- [ ] Packaging, install, or uninstall
- [ ] CI or build tooling
- [ ] Breaking change

## Areas touched

- [ ] `gazed` daemon or DBus interface
- [ ] `gaze` CLI
- [ ] `gaze-gui`
- [ ] PAM modules (`pam-gaze`, `pam-gaze-grosshack`)
- [ ] GNOME extension or GDM
- [ ] KDE Plasma integration
- [ ] hyprlock, LightDM, or TTY console
- [ ] Camera, IR, or liveness
- [ ] Recognition models or thresholds
- [ ] Config parsing or defaults
- [ ] Packaging, systemd units, or polkit policy
- [ ] Docs

## Verification

**Required checks.** Paste the result, do not just tick the box.

```
just fmt-check
just lint
just test
just audit
```

<!-- If you changed the Justfile, also `just --fmt --check`. -->
<!-- If you changed packaging, scripts, units, DBus policy, PAM, or the GNOME extension, also `just package <deb|rpm|archlinux>`. -->

<details>
<summary>Output</summary>

```text

```

</details>

**Tests.**

- [ ] Added or updated tests for the changed behavior
- [ ] Existing tests cover this
- [ ] Not testable in CI without hardware or system services, manual steps are below

**Manual verification.** Required if this touches hardware, DBus, PAM, a greeter,
a lock screen, or packaging. Give the exact steps someone else can follow, plus what
you ran it on.

<!--
Tested on: <distro + version>, <desktop + version>, <Wayland/X11>, <display manager>, <camera model>

1.
2.
3.

Result:
-->

## PAM safety

<!--
Delete this section if you did not touch PAM.

- [ ] I had a second root shell or TTY open while testing
- [ ] I tested against a non-critical PAM service before `sudo`, `system-auth`, or my graphical login
- [ ] Password fallback still works when face auth fails, and when `gazed` is stopped entirely
- [ ] Exact reproduction steps are in the manual verification section above

Unsafe FFI touched: yes / no. If yes, say what invariant keeps it sound.
-->

## Docs

- [ ] Docs updated, because this changes user-visible behavior, install steps, config, CLI output, or packaging
- [ ] No docs change needed

## Licensing

- [ ] New files carry the SPDX header used throughout the tree
- [ ] No code was copied from a project whose license is incompatible with GPL-3.0-or-later
- [ ] Any new dependency is license-compatible and I checked it myself, since `just audit` does not
- [ ] I agree that my contribution is licensed under `GPL-3.0-or-later`

## AI disclosure

We accept AI-assisted contributions and review them on the same terms as any other.
Using AI is not disqualifying. Not telling us is, because it changes what a reviewer
has to check: a human who misreads a PAM lifetime makes a mistake we can spot from
the surrounding reasoning, while a model that fabricates one produces a diff that
reads correct all the way down.

**Select exactly one.**

- [ ] **None.** I wrote this by hand.
- [ ] **Assisted.** AI helped with parts of it: completion, boilerplate, docs prose,
      test scaffolding, or naming. I directed the design and read every line.
- [ ] **Generated.** AI produced most of the diff from my prompting. I reviewed all
      of it and I understand it.
- [ ] **Agentic.** An AI agent produced this largely on its own, with limited
      supervision from me.

**Tools used:** <!-- e.g. Claude Code, GitHub Copilot, Cursor, Codex, aider. Write "none" if none. -->

**Where it helped, and where you had to correct it:**

<!--
One or two sentences. This is the most useful part of this section, so please do not
skip it if you ticked anything other than "None". Knowing that the model got the
happy path right but invented the error handling tells a reviewer exactly where to
look first.
-->

**Confirm, whichever option you picked:**

- [ ] Every check result and manual test above was produced by actually running it on
      a real machine. None of it is predicted, assumed, inferred from reading the code,
      or reported to me by an AI tool.
- [ ] I have read every line of this diff and can explain why each change is here.
- [ ] I did not paste proprietary, confidential, or license-incompatible code into an
      AI tool to produce this, and the output does not reproduce such code.
- [ ] I will respond to review feedback myself rather than forwarding it to a tool
      unread.

## Follow-ups and known limitations

<!--
What you deliberately did not do, what you are unsure about, and what should be
reviewed hardest. Saying "I could not test the GDM path, I have no GNOME machine"
is far more useful than silence, and it is not a mark against the pull request.
-->
