#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
"""Source regression check; Nix evaluation and live PAM/greeter tests are still required."""
from pathlib import Path

source = (Path(__file__).resolve().parents[1] / "packaging/nix/nixos-module.nix").read_text()
login = source.split("(lib.mkIf cfg.kde.loginScreen {", 1)[1].split("(lib.mkIf cfg.gnome.enable", 1)[0]
wallet, without_wallet = login.split("else ''", 1)
for branch in (wallet, without_wallet):
    rules = [line.split() for line in branch.splitlines()]
    assert ["account", "include", "plasmalogin"] in rules
    assert ["session", "include", "plasmalogin"] in rules
    assert not any("pam_permit.so" in line for line in rules)
print("PASS: both biometric login branches include normal account/session policy")
