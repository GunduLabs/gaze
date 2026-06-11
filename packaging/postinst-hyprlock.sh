#!/bin/sh
set -e

cat <<'EOF'

Gaze hyprlock PAM service installed at /etc/pam.d/hyprlock-gaze
To enable face unlock in hyprlock, add to ~/.config/hypr/hyprlock.conf:

    general {
        pam_module = hyprlock-gaze
    }

For simultaneous face + password mode, use:

        pam_module = hyprlock-gaze-simultaneous

Docs: https://gaze.gundulabs.com/guide/hyprland
EOF
