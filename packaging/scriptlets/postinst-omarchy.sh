#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e
cat <<'EOF'

Gaze Omarchy integration installed. From your unlocked desktop, without sudo:

    gaze-omarchy enable
    gaze add-face default
    gaze auth
    gaze-omarchy doctor

After package or Omarchy updates, run gaze-omarchy doctor. Reload the plugin
with gaze-omarchy enable while unlocked. Before removing this package, run
gaze-omarchy disable to restore the stock lock.

Docs: https://gaze.gundulabs.com/guide/omarchy
EOF
