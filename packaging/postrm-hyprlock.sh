#!/bin/sh
set -e

cat <<'EOF'

Gaze hyprlock PAM service removed.
If your hyprlock.conf still references module = hyprlock-gaze, hyprlock
will fall back to its default PAM service. Update hyprlock.conf to remove
the reference.
EOF
