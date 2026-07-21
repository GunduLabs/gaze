#!/bin/sh
set -e

cat <<'EOF'

Gaze KDE integration installed at /etc/pam.d/kde-fingerprint.
Face authentication now runs on the KDE Plasma lock screen and login
greeter, in parallel with the password field, so a face match unlocks
without pressing a key. Lock your screen and look at the camera.

If /etc/pam.d/kde-fingerprint already existed, your file was kept and the
packaged one was saved alongside it (.rpmnew / .pacnew, or a dpkg prompt);
merge in the pam_gaze.so auth line to enable face unlock.

Docs: https://gaze.gundulabs.com/guide/kde
EOF
