#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e
cat <<'EOF'

Gaze Omarchy integration installed. From your unlocked desktop, without sudo:

    gaze-omarchy enable
    gaze add-face default
    gaze test
    gaze-omarchy doctor

After package or Omarchy updates, run gaze-omarchy doctor. Reload the plugin
with gaze-omarchy enable while unlocked. Before removing this package, run
gaze-omarchy disable to restore the stock lock.

Docs: https://gaze.gundulabs.com/guide/omarchy
EOF

# The plugin directory links into this package, so an upgrade swaps a live locker's files.
if [ ! -f /usr/share/omarchy/shell/plugins/lock/FingerprintModel.js ]; then
    for gaze_omarchy_home in /home/* /root; do
        gaze_omarchy_link="$gaze_omarchy_home/.config/omarchy/plugins/com.gundulabs.gaze.lock"
        if [ -L "$gaze_omarchy_link" ] &&
            [ "$(readlink "$gaze_omarchy_link")" = /usr/share/gaze/omarchy ]; then
            cat >&2 <<EOF

WARNING: this gaze-omarchy needs Omarchy 4.5.0 or newer, but an older Omarchy
is installed and the Gaze lock is enabled for $gaze_omarchy_home.
The lock screen may not load. From the unlocked desktop, either update Omarchy
and run gaze-omarchy enable, or run gaze-omarchy disable to restore the stock lock.
EOF
        fi
    done
fi
