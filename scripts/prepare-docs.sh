#!/usr/bin/env bash
# Download archived docs for versioned tags
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

echo "Preparing versioned documentation..."

git fetch --tags || true

rm -rf docs/archive
mkdir -p docs/archive

tags=$(git tag -l "v*")

for tag in $tags; do
  echo "Extracting docs for tag $tag..."

  tmp_dir="docs/archive/tmp_$tag"
  rm -rf "$tmp_dir"
  mkdir -p "$tmp_dir"

  git archive --format=tar "$tag" docs 2>/dev/null | tar -x -C "$tmp_dir" --strip-components=1 2>/dev/null || {
    echo "No docs folder found in tag $tag, skipping."
    rm -rf "$tmp_dir"
    continue
  }

  index_file=$(find "$tmp_dir" -name "index.md" | head -n 1)

  if [ -n "$index_file" ]; then
    docs_root=$(dirname "$index_file")
    mkdir -p "docs/archive/$tag"
    cp -r "$docs_root"/* "docs/archive/$tag/"
  else
    echo "Could not find index.md in the docs folder of tag $tag, skipping."
  fi

  rm -rf "$tmp_dir"
done

echo "Done preparing versioned documentation."
