#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ARTIFACTS_DIR="${ARTIFACTS_DIR:-$ROOT_DIR/dist/packages}"
AUR_GENERATED_DIR="${AUR_GENERATED_DIR:-$ROOT_DIR/dist/aur}"
PROJECT_URL="${PROJECT_URL:-https://github.com/GunduLabs/gaze}"
RELEASE_TAG="${RELEASE_TAG:-}"
PKGBUILD_TEMPLATE="$ROOT_DIR/packaging/aur/PKGBUILD.tpl"

if [[ -z "$RELEASE_TAG" ]]; then
  echo "RELEASE_TAG is required to build package source URLs"
  exit 1
fi

AUR_SOURCE_BASE_URL="${AUR_SOURCE_BASE_URL:-$PROJECT_URL/releases/download/$RELEASE_TAG}"

mkdir -p "$AUR_GENERATED_DIR"

mapfile -d '' -t artifact_pkgs < <(
  find "$ARTIFACTS_DIR" -type f \
    \( -name '*.pkg.tar.zst' -o -name '*.pkg.tar.xz' -o -name '*.pkg.tar.gz' -o -name '*.pkg.tar.bz2' \) \
    -print0
)

if [[ ${#artifact_pkgs[@]} -eq 0 ]]; then
  echo "No Arch package artifacts found in $ARTIFACTS_DIR"
  exit 1
fi

quote_bash_array() {
  local -a values=("$@")
  if [[ ${#values[@]} -eq 0 ]]; then
    printf "()"
    return
  fi

  printf "("
  local value
  for value in "${values[@]}"; do
    printf "'%s' " "${value//\'/\'\\\'\'}"
  done
  printf ")"
}

extract_single_field() {
  local pkg_file="$1"
  local field_name="$2"

  bsdtar -xOf "$pkg_file" .PKGINFO | awk -F ' = ' -v key="$field_name" '$1 == key { print $2; exit }'
}

extract_multi_field() {
  local pkg_file="$1"
  local field_name="$2"

  bsdtar -xOf "$pkg_file" .PKGINFO | awk -F ' = ' -v key="$field_name" '$1 == key { print $2 }'
}

for pkg_file in "${artifact_pkgs[@]}"; do
  file_name="$(basename "$pkg_file")"

  pkgname="$(extract_single_field "$pkg_file" "pkgname")"
  pkgver="$(extract_single_field "$pkg_file" "pkgver")"
  pkgrel="$(extract_single_field "$pkg_file" "pkgrel")"

  if [[ -z "$pkgrel" ]]; then
    pkgver_parts=()
    IFS='-' read -r -a pkgver_parts <<< "$pkgver"
    if [[ ${#pkgver_parts[@]} -ge 2 ]]; then
      pkgver="${pkgver_parts[0]}"
      pkgrel="${pkgver_parts[1]}"
    fi
  fi

  if [[ -z "$pkgname" || -z "$pkgver" || -z "$pkgrel" ]]; then
    echo "Skipping $file_name because required metadata is missing"
    continue
  fi

  aur_pkgname="${pkgname}-bin"
  aur_dir="$AUR_GENERATED_DIR/$aur_pkgname"
  mkdir -p "$aur_dir"

  mapfile -t arches < <(extract_multi_field "$pkg_file" "arch" | sort -u)
  mapfile -t depends < <(extract_multi_field "$pkg_file" "depend" | sort -u)
  checksum="$(sha256sum "$pkg_file" | awk '{print $1}')"

  export AUR_PKGNAME="$aur_pkgname"
  export AUR_PKGVER="$pkgver"
  export AUR_PKGREL="$pkgrel"
  export AUR_BASENAME="$pkgname"
  export AUR_ARCH="$(quote_bash_array "${arches[@]}")"
  export AUR_DEPENDS="$(quote_bash_array "${depends[@]}")"
  export AUR_FILE="$file_name"
  export AUR_URL="$PROJECT_URL"
  export AUR_SOURCE_URL="$AUR_SOURCE_BASE_URL/$file_name"
  export AUR_SHA256="$checksum"

  envsubst '$AUR_PKGNAME $AUR_PKGVER $AUR_PKGREL $AUR_BASENAME $AUR_ARCH $AUR_DEPENDS $AUR_FILE $AUR_URL $AUR_SOURCE_URL $AUR_SHA256' \
    < "$PKGBUILD_TEMPLATE" > "$aur_dir/PKGBUILD"

  (cd "$aur_dir" && makepkg --printsrcinfo > .SRCINFO)

  echo "Generated AUR wrapper for $aur_pkgname from $file_name"
done

if ! find "$AUR_GENERATED_DIR" -mindepth 1 -maxdepth 1 -type d | grep -q .; then
  echo "No AUR package wrappers were generated"
  exit 1
fi

echo "AUR wrappers generated in $AUR_GENERATED_DIR"
