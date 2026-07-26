#!/bin/sh
set -e

if [ -x /usr/bin/gaze-cosmic-pam ]; then
	gaze_cosmic_status=0
	/usr/bin/gaze-cosmic-pam enable || gaze_cosmic_status=$?
else
	gaze_cosmic_status=1
fi

if [ "$gaze_cosmic_status" -eq 0 ]; then
	cat <<'EOF'

Gaze face authentication is enabled for COSMIC.

  Lock screen: locks now use face unlock, falling back to your password
  Login screen: the same PAM service backs the cosmic-greeter login screen

Turn it off again with:

    sudo gaze-cosmic-pam disable

Docs: https://gaze.gundulabs.com/guide/cosmic
EOF
else
	cat <<'EOF'

Gaze could not wire itself into /etc/pam.d/cosmic-greeter.

Install cosmic-greeter, then run:

    sudo gaze-cosmic-pam enable

Docs: https://gaze.gundulabs.com/guide/cosmic
EOF
fi
