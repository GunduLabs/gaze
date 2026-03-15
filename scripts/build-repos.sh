#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-}"
PACKAGE_DIR="${2:-dist/packages}"
OUTPUT_DIR="${3:-dist/repos}"

if [[ -z "$MODE" ]]; then
  echo "usage: $0 <deb-rpm|arch|all> [package_dir] [output_dir]" >&2
  exit 1
fi

mkdir -p "$OUTPUT_DIR"

copy_signing_artifacts() {
  shopt -s nullglob
  for artifact in "$PACKAGE_DIR"/PACKAGE-SIGNING-KEY.asc "$PACKAGE_DIR"/SHA256SUMS "$PACKAGE_DIR"/SHA256SUMS.sig; do
    if [[ -f "$artifact" ]]; then
      cp -f "$artifact" "$OUTPUT_DIR/"
    fi
  done
  shopt -u nullglob
}

sign_file() {
  local input="$1"
  local output="$2"

  if [[ -z "${GPG_SIGN_KEY_ID:-}" ]]; then
    return 0
  fi

  local gpg_args=(--batch --yes --pinentry-mode loopback)
  if [[ -n "${GPG_HOMEDIR:-}" ]]; then
    gpg_args+=(--homedir "$GPG_HOMEDIR")
  fi
  if [[ -n "${GPG_PASSPHRASE:-}" ]]; then
    gpg_args+=(--passphrase "$GPG_PASSPHRASE")
  fi

  gpg "${gpg_args[@]}" \
    --local-user "$GPG_SIGN_KEY_ID" \
    --detach-sign \
    --output "$output" \
    "$input"
}

sign_clearsigned() {
  local input="$1"
  local output="$2"

  if [[ -z "${GPG_SIGN_KEY_ID:-}" ]]; then
    return 0
  fi

  local gpg_args=(--batch --yes --pinentry-mode loopback)
  if [[ -n "${GPG_HOMEDIR:-}" ]]; then
    gpg_args+=(--homedir "$GPG_HOMEDIR")
  fi
  if [[ -n "${GPG_PASSPHRASE:-}" ]]; then
    gpg_args+=(--passphrase "$GPG_PASSPHRASE")
  fi

  gpg "${gpg_args[@]}" \
    --local-user "$GPG_SIGN_KEY_ID" \
    --clearsign \
    --output "$output" \
    "$input"
}

build_deb_repo() {
  mkdir -p "$OUTPUT_DIR/deb/pool/main" "$OUTPUT_DIR/deb/dists/stable/main/binary-amd64"

  shopt -s nullglob
  local debs=("$PACKAGE_DIR"/*.deb)
  shopt -u nullglob

  if [[ ${#debs[@]} -eq 0 ]]; then
    echo "No .deb packages found in $PACKAGE_DIR; skipping deb repo generation."
    return 0
  fi

  command -v dpkg-scanpackages >/dev/null 2>&1 || {
    echo "dpkg-scanpackages is required to build deb repo metadata" >&2
    exit 1
  }
  command -v apt-ftparchive >/dev/null 2>&1 || {
    echo "apt-ftparchive is required to build deb repo metadata" >&2
    exit 1
  }

  cp -f "$PACKAGE_DIR"/*.deb "$OUTPUT_DIR/deb/pool/main/"
  shopt -s nullglob
  for sig in "$PACKAGE_DIR"/*.deb.sig; do
    cp -f "$sig" "$OUTPUT_DIR/deb/pool/main/"
  done
  shopt -u nullglob

  pushd "$OUTPUT_DIR/deb" >/dev/null
  dpkg-scanpackages --multiversion pool /dev/null > dists/stable/main/binary-amd64/Packages
  gzip -9c dists/stable/main/binary-amd64/Packages > dists/stable/main/binary-amd64/Packages.gz
  apt-ftparchive release dists/stable > dists/stable/Release
  sign_file dists/stable/Release dists/stable/Release.gpg
  sign_clearsigned dists/stable/Release dists/stable/InRelease
  popd >/dev/null
}

build_rpm_repo() {
  mkdir -p "$OUTPUT_DIR/rpm/x86_64"

  shopt -s nullglob
  local rpms=("$PACKAGE_DIR"/*.rpm)
  shopt -u nullglob

  if [[ ${#rpms[@]} -eq 0 ]]; then
    echo "No .rpm packages found in $PACKAGE_DIR; skipping rpm repo generation."
    return 0
  fi

  command -v createrepo_c >/dev/null 2>&1 || {
    echo "createrepo_c is required to build rpm repo metadata" >&2
    exit 1
  }

  cp -f "$PACKAGE_DIR"/*.rpm "$OUTPUT_DIR/rpm/x86_64/"
  shopt -s nullglob
  for sig in "$PACKAGE_DIR"/*.rpm.sig; do
    cp -f "$sig" "$OUTPUT_DIR/rpm/x86_64/"
  done
  shopt -u nullglob
  createrepo_c --update "$OUTPUT_DIR/rpm/x86_64"
  sign_file "$OUTPUT_DIR/rpm/x86_64/repodata/repomd.xml" "$OUTPUT_DIR/rpm/x86_64/repodata/repomd.xml.asc"
}

build_arch_repo() {
  mkdir -p "$OUTPUT_DIR/arch/x86_64"

  shopt -s nullglob
  local arch_pkgs=()
  for pkg in "$PACKAGE_DIR"/*.pkg.tar.*; do
    if [[ "$pkg" == *.sig ]]; then
      continue
    fi
    arch_pkgs+=("$pkg")
  done
  shopt -u nullglob

  if [[ ${#arch_pkgs[@]} -eq 0 ]]; then
    echo "No Arch packages found in $PACKAGE_DIR; skipping arch repo generation."
    return 0
  fi

  command -v repo-add >/dev/null 2>&1 || {
    echo "repo-add is required to build arch repo metadata" >&2
    exit 1
  }

  cp -f "${arch_pkgs[@]}" "$OUTPUT_DIR/arch/x86_64/"
  repo-add "$OUTPUT_DIR/arch/x86_64/gaze.db.tar.gz" "$OUTPUT_DIR/arch/x86_64"/*.pkg.tar.*
  shopt -s nullglob
  for sig in "$PACKAGE_DIR"/*.pkg.tar.*.sig; do
    cp -f "$sig" "$OUTPUT_DIR/arch/x86_64/"
  done
  shopt -u nullglob
  sign_file "$OUTPUT_DIR/arch/x86_64/gaze.db.tar.gz" "$OUTPUT_DIR/arch/x86_64/gaze.db.tar.gz.sig"
  sign_file "$OUTPUT_DIR/arch/x86_64/gaze.files.tar.gz" "$OUTPUT_DIR/arch/x86_64/gaze.files.tar.gz.sig"
}

case "$MODE" in
  deb-rpm)
    copy_signing_artifacts
    build_deb_repo
    build_rpm_repo
    ;;
  arch)
    copy_signing_artifacts
    build_arch_repo
    ;;
  all)
    copy_signing_artifacts
    build_deb_repo
    build_rpm_repo
    build_arch_repo
    ;;
  *)
    echo "unknown mode: $MODE" >&2
    exit 1
    ;;
esac
