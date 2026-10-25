#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check the shipped face stack and readiness contract (not a real PAM tally test)."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
stack = root / "packaging/pam/gaze-omarchy-face"
script = (root / "integrations/omarchy/gaze-omarchy").read_text()
ready = script[script.index("pam_ready() {"):script.index("\nowns_link() {")]
with tempfile.TemporaryDirectory() as tmp:
    fixture = Path(tmp) / "face"
    fixture.write_text(stack.read_text())
    harness = 'trusted_file() { return 0; }\n' + ready + '\nPAM_FILE="$1"\npam_ready\n'
    subprocess.run(["sh", "-ec", harness, "test", str(fixture)], check=True)
    fixture.write_text(stack.read_text().replace("pam_faillock.so authfail", "pam_deny.so"))
    assert subprocess.run(["sh", "-ec", harness, "test", str(fixture)]).returncode != 0

rules = [line.split() for line in stack.read_text().splitlines()
         if line.startswith("auth ")]
assert rules[2] == ["auth", "[success=2", "auth_err=ignore", "default=1]", "pam_gaze.so"]
assert rules[3] == ["auth", "[default=die]", "pam_faillock.so", "authfail"]
assert rules[4] == ["auth", "requisite", "pam_deny.so"]
assert rules[5] == ["auth", "required", "pam_faillock.so", "authsucc"]
# success skips authfail + deny; auth_err records a failure; other results skip authfail.
print("PASS: readiness contract and result-sensitive PAM control layout")
