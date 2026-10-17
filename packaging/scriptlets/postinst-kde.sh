#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e

if [ -x /usr/bin/gaze-kde-pam ]; then
	gaze_kde_status=0
	/usr/bin/gaze-kde-pam enable || gaze_kde_status=$?
else
	gaze_kde_status=1
fi

if [ "$gaze_kde_status" -eq 0 ]; then
	cat <<'EOF'

Gaze face unlock is enabled on the KDE Plasma lock screen.

It runs in the slot KScreenLocker starts up front for biometrics, alongside the
password field, so a face match unlocks without pressing a key. Lock your screen
and look at the camera.

The login greeter is separate and off by default. Unless your Plasma Login Manager
ships an up-front biometric service, face auth there starts when you submit the
login form, exactly as a fingerprint reader does on that screen. Turn it on with:

    sudo gaze-kde-pam enable-login

Turn face unlock off again with:

    sudo gaze-kde-pam disable

Docs: https://gaze.gundulabs.com/guide/kde
EOF
else
	cat <<'EOF'

Gaze could not wire itself into KScreenLocker's biometric slot.

Run this once KDE Plasma is installed:

    sudo gaze-kde-pam enable

Docs: https://gaze.gundulabs.com/guide/kde
EOF
fi
