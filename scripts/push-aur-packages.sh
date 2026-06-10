#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUR_GENERATED_DIR="${AUR_GENERATED_DIR:-$ROOT_DIR/dist/aur}"
AUR_WORKDIR="${AUR_WORKDIR:-$ROOT_DIR/dist/aur-work}"

if [[ ! -d "$AUR_GENERATED_DIR" ]]; then
  echo "AUR generated directory does not exist: $AUR_GENERATED_DIR"
  exit 1
fi

mkdir -p "$AUR_WORKDIR"

mapfile -t aur_packages < <(find "$AUR_GENERATED_DIR" -mindepth 1 -maxdepth 1 -type d | sort)
if [[ ${#aur_packages[@]} -eq 0 ]]; then
  echo "No generated AUR package directories found"
  exit 1
fi

for generated_dir in "${aur_packages[@]}"; do
  aur_pkgname="$(basename "$generated_dir")"
  local_clone="$AUR_WORKDIR/$aur_pkgname"
  remote_url="ssh://aur@aur.archlinux.org/$aur_pkgname.git"

  if [[ ! -d "$local_clone/.git" ]]; then
    rm -rf "$local_clone"
    git clone -c init.defaultBranch=master "$remote_url" "$local_clone"
  else
    git -C "$local_clone" remote set-url origin "$remote_url"

    if ! git -C "$local_clone" diff --quiet || ! git -C "$local_clone" diff --cached --quiet; then
      rm -rf "$local_clone"
      git clone -c init.defaultBranch=master "$remote_url" "$local_clone"
    else
      git -C "$local_clone" fetch origin
      if git -C "$local_clone" show-ref --verify --quiet refs/remotes/origin/master; then
        git -C "$local_clone" checkout -B master origin/master
      else
        echo "Remote $aur_pkgname is empty (no origin/master yet); creating initial master on first push"
      fi
    fi
  fi

  git -C "$local_clone" config user.name "github-actions[bot]"
  git -C "$local_clone" config user.email "41898282+github-actions[bot]@users.noreply.github.com"

  rsync -a --delete --exclude '.git/' "$generated_dir/" "$local_clone/"

  git -C "$local_clone" add -A
  if git -C "$local_clone" diff --cached --quiet; then
    echo "No AUR changes for $aur_pkgname"
    continue
  fi

  pkgver="$(awk -F '=' '/^pkgver=/{print $2; exit}' "$local_clone/PKGBUILD" | tr -d ' ')"
  pkgrel="$(awk -F '=' '/^pkgrel=/{print $2; exit}' "$local_clone/PKGBUILD" | tr -d ' ')"

  git -C "$local_clone" commit -m "chore: update to $pkgver-$pkgrel"
  git -C "$local_clone" push origin HEAD:master

  echo "Pushed AUR update for $aur_pkgname"
done
