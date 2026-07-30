#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e
pam-auth-update --package --remove gaze
pam-auth-update --package --remove gaze-simultaneous
