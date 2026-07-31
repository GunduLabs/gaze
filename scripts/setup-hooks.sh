#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
# Setup git hooks for Gaze
set -eu

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOKS_PATH="scripts"

if [ ! -x "$REPO_ROOT/$HOOKS_PATH/pre-commit" ]; then
    echo "Error: $HOOKS_PATH/pre-commit is missing or not executable." >&2
    exit 1
fi

echo "Setting up git hooks..."
git config core.hooksPath "$HOOKS_PATH"

echo "Done! Git hooks are now active from $HOOKS_PATH/."
