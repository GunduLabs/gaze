#!/bin/sh
# Setup git hooks for Gaze

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
GIT_DIR="$(git rev-parse --git-dir)"

echo "Setting up pre-commit hook..."
cp "$SCRIPT_DIR/pre-commit" "$GIT_DIR/hooks/pre-commit"
chmod +x "$GIT_DIR/hooks/pre-commit"

echo "Done! Pre-commit hooks are now active."
