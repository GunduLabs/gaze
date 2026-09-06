# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Justfile for Gaze: https://gaze.gundulabs.com
# Run `just` to see available targets.

set lazy

# Host architecture from just; can be overridden: just arch=aarch64 package deb
arch := env("ARCH", arch())
# Package version; defaults to the git tag (strip leading v)
version := if env("VERSION", "") != "" { env("VERSION") } else { trim_start_match(shell("git describe --tags --always"), "v") }
# Package release/revision; distro builds set this to keep artifacts distinct.
package_release := env("PACKAGE_RELEASE", "1")
# Required packaging tool; evaluated only by packaging recipes.
nfpm := require("nfpm")
# Required by the `srpm` recipe only.
rpmbuild := require("rpmbuild")
# ONNX Runtime release bundled into the offline builds (flatpak and srpm).
ort_version := env("ORT_VERSION", "1.29.0")

# The opencv crate probes only the `opencv4`/`opencv` pkg-config names, so distros shipping
# OpenCV 5 (e.g. Arch) need this override. Empty when opencv4/opencv resolve or opencv5 doesn't.
opencv_env := shell("pkg-config --exists opencv4 2>/dev/null || pkg-config --exists opencv 2>/dev/null || ! pkg-config --exists opencv5 2>/dev/null || echo OPENCV_PKGCONFIG_NAME=opencv5")

# Build the GTK front-end (`gaze-gui`). `GAZE_GUI=0`/`false`/`no`/`off` drops it,
# and gtk4/libadwaita, from the build, test, and lint recipes below only.
gui := lowercase(env("GAZE_GUI", "1"))
gui_off := if gui =~ '^(0|false|no|off)$' { "1" } else { "" }
gui_pkg := if gui_off == "1" { "" } else { "-p gaze-gui" }
gui_feature := if gui_off == "1" { "" } else { ",gaze-gui/openvino" }
gui_exclude := if gui_off == "1" { "--exclude gaze-gui" } else { "" }
gui_notice := if gui_off == "1" { "echo 'note: GAZE_GUI is off, gaze-gui is excluded here; CI still checks it'" } else { "true" }

# Derived vars
multiarch := if arch == "aarch64" { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" }
deb_arch := if arch == "x86_64" { "amd64" } else if arch == "aarch64" { "arm64" } else { arch }

# List recipes when `just` is run without arguments.
[default]
[private]
default:
    @{{ quote(just_executable()) }} --justfile {{ quote(justfile()) }} --list

# ── build ─────────────────────────────────────────────────────────────────────

# Two invocations so gaze-core's `detection` feature does not unify into the client binaries;
# ONNX Runtime's constructors require AVX2 and crash on older CPUs.
# Build all Rust workspace binaries (release)
[group("build")]
build-rust:
    {{ opencv_env }} cargo build -p gaze --release
    {{ opencv_env }} cargo build -p gaze-cli {{ gui_pkg }} -p pam-gaze -p pam-gaze-grosshack --release

# Build all Rust workspace binaries with OpenVINO configuration and runtime support.
[group("build")]
build-rust-openvino:
    {{ opencv_env }} cargo build -p gaze --release --features gaze/openvino
    {{ opencv_env }} cargo build -p gaze-cli {{ gui_pkg }} -p pam-gaze -p pam-gaze-grosshack --release --features gaze-cli/openvino{{ gui_feature }}

# Compile the SELinux policy module
[group("build")]
build-selinux:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p dist/selinux
    if command -v checkmodule >/dev/null 2>&1 && command -v semodule_package >/dev/null 2>&1; then
        checkmodule -M -m -o dist/selinux/gaze-gdm-camera.mod packaging/selinux/gaze-gdm-camera.te
        semodule_package -o dist/selinux/gaze-gdm-camera.pp -m dist/selinux/gaze-gdm-camera.mod
        rm -f dist/selinux/gaze-gdm-camera.mod
        echo "Built dist/selinux/gaze-gdm-camera.pp"
    else
        echo "WARNING: SELinux tools not found. Skipping SELinux policy build." >&2
    fi

[private]
prepare-flatpak-vendor:
    mkdir -p .flatpak-cache/cargo
    cargo vendor --locked --versioned-dirs > .flatpak-cache/cargo/config.toml

[private]
prepare-flatpak-ort:
    mkdir -p .flatpak-cache/ort
    arch="$(flatpak --default-arch)"; \
    case "$arch" in \
        x86_64) ort_arch="x64" ;; \
        aarch64) ort_arch="aarch64" ;; \
        *) echo "Unsupported Flatpak arch for ORT bootstrap: $arch" >&2; exit 1 ;; \
    esac; \
    ort_file="onnxruntime-linux-${ort_arch}-{{ ort_version }}.tgz"; \
    ort_url="https://github.com/microsoft/onnxruntime/releases/download/v{{ ort_version }}/${ort_file}"; \
    if [ ! -s .flatpak-cache/ort/onnxruntime.tgz ]; then \
        curl -fsSL "$ort_url" -o .flatpak-cache/ort/onnxruntime.tgz; \
    fi

# flatpak-builder's ostree dirs need xattrs and same-filesystem co-location, so they default to
# the repo tree; the `docker` wrapper moves them into a volume the sshfs mount can't host.
flatpak_state_dir := env("FLATPAK_STATE_DIR", ".flatpak-builder")
flatpak_build_dir := env("FLATPAK_BUILD_DIR", "flatpak-build")
flatpak_repo_dir := env("FLATPAK_REPO_DIR", "dist/flatpak-repo")

# Build flatpak repo and bundle
[group("build")]
build-flatpak: prepare-flatpak-vendor prepare-flatpak-ort
    mkdir -p dist/packages {{ quote(flatpak_repo_dir) }}
    flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo

    flatpak-builder \
        --force-clean \
        --disable-rofiles-fuse \
        --install-deps-from=flathub \
        --state-dir={{ quote(flatpak_state_dir) }} \
        --jobs="${FLATPAK_BUILDER_JOBS:-2}" \
        --repo={{ quote(flatpak_repo_dir) }} \
        --arch="$(flatpak --default-arch)" \
        --default-branch=stable \
        --user \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        {{ quote(flatpak_build_dir) }} \
        packaging/flatpak/com.gundulabs.Gaze.yml

    arch="$(flatpak --default-arch)"; \
    flatpak build-bundle \
        --arch="$arch" \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        {{ quote(flatpak_repo_dir) }} \
        "dist/packages/com.gundulabs.Gaze-${arch}.flatpak" \
        com.gundulabs.Gaze \
        stable

    install -Dm644 packaging/flatpak/com.gundulabs.Gaze.flatpakref dist/packages/com.gundulabs.Gaze.flatpakref
    install -Dm644 packaging/flatpak/com.gundulabs.Gaze.flatpakrepo dist/packages/com.gundulabs.Gaze.flatpakrepo
    if [ -n "${FLATPAK_GPG_SIGN:-}" ]; then \
        pubkey="$(gpg --export "${FLATPAK_GPG_SIGN}" | base64 -w0)"; \
        for f in dist/packages/com.gundulabs.Gaze.flatpakref dist/packages/com.gundulabs.Gaze.flatpakrepo; do \
            [ -s "$f" ] && [ "$(tail -c1 "$f" | od -An -tx1 | tr -d ' \n')" != "0a" ] \
                && printf '\n' >> "$f" || true; \
            printf 'GPGKey=%s\n' "$pubkey" >> "$f"; \
        done; \
    fi

# ── package ───────────────────────────────────────────────────────────────────

[arg("format", pattern="deb|rpm|archlinux")]
[env("MULTIARCH", multiarch)]
[env("PACKAGE_RELEASE", package_release)]
[env("VERSION", version)]
[private]
_nfpm config format:
    #!/usr/bin/env bash
    set -euo pipefail
    export ARCH="{{ if format == "deb" { deb_arch } else { arch } }}"

    binaries=()
    while read -r src; do
        [ -n "$src" ] || continue
        [ -f "$src" ] || { echo "_nfpm: {{ config }} ships $src, which has not been built" >&2; exit 1; }
        binaries+=("$src")
    done < <(grep -oE 'target/release/[A-Za-z0-9_.+-]+' {{ quote(config) }} | sort -u)

    needed() { objdump -p "${binaries[@]}" | awk '/NEEDED/ { print $2 }' | sort -u; }
    yaml_list() { sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e '/^$/d' -e 's/^/      - /'; }

    lib_depends=""
    if [ "${#binaries[@]}" -gt 0 ]; then
        case "{{ format }}" in
        deb)
            command -v dpkg-shlibdeps >/dev/null 2>&1 || {
                echo "_nfpm: dpkg-shlibdeps (dpkg-dev) is required to build deb packages; run inside a Debian/Ubuntu environment, e.g. 'just docker package-prebuilt deb'" >&2
                exit 1
            }
            scaffold=$(mktemp -d)
            mkdir -p "$scaffold/debian"
            printf 'Source: gaze\n\nPackage: gaze\nArchitecture: any\nDescription: dpkg-shlibdeps scaffolding\n' \
                > "$scaffold/debian/control"
            absolute=()
            for binary in "${binaries[@]}"; do absolute+=("$PWD/$binary"); done
            field=$(cd "$scaffold" && dpkg-shlibdeps -O --warnings=0 "${absolute[@]}")
            rm -rf "$scaffold"
            lib_depends=$(printf '%s\n' "${field#*shlibs:Depends=}" | tr ',' '\n' | yaml_list)
            ;;
        rpm)
            isa=""
            objdump -f "${binaries[0]}" | grep -q 'file format elf64' && isa="()(64bit)"
            requires=""
            while read -r soname; do
                if rpm -q --whatprovides "${soname}${isa}" >/dev/null 2>&1; then
                    requires+="${soname}${isa}"$'\n'
                else
                    echo "_nfpm: no installed package provides ${soname}${isa}; leaving it out of Requires" >&2
                fi
            done < <(needed)
            lib_depends=$(printf '%s' "$requires" | yaml_list)
            ;;
        archlinux)
            # Arch bumps the OpenCV soname every minor release, so pin the soversion the shipped
            # binaries linked; unpinned, an upgrade crash-loops the daemon instead of failing.
            opencv_soname=$(needed | grep '^libopencv_core\.so\.' || true)
            if [ -n "$opencv_soname" ]; then
                sover=${opencv_soname##*.so.}
                [[ "$sover" =~ ^[0-9]+$ ]] || { echo "_nfpm: cannot read libopencv_core soversion from {{ config }}" >&2; exit 1; }
                export OPENCV_MIN="$((sover / 100)).$((sover % 100))"
                export OPENCV_NEXT="$((sover / 100)).$((sover % 100 + 1))"
            fi
            ;;
        esac

        if [ "{{ format }}" != "archlinux" ]; then
            [ -n "$lib_depends" ] || {
                echo "_nfpm: resolved no library dependencies for {{ config }} ({{ format }})" >&2
                exit 1
            }
            if needed | grep -q '^libopencv_core\.so\.'; then
                opencv_pattern='libopencv-core[0-9]'
                [ "{{ format }}" = "rpm" ] && opencv_pattern='libopencv_core\.so\.[0-9]'
                grep -Eq "$opencv_pattern" <<< "$lib_depends" || {
                    echo "_nfpm: generated {{ format }} dependencies for {{ config }} do not pin the OpenCV soversion" >&2
                    exit 1
                }
            fi
        fi
    fi

    is_suse() {
        local os_rel=""
        for os_rel in /etc/os-release /usr/lib/os-release; do
            [ -r "$os_rel" ] || continue
            (
                # shellcheck disable=SC1091
                . "$os_rel"
                case "${ID:-} ${ID_LIKE:-}" in
                *opensuse*|*suse*) exit 0 ;;
                *) exit 1 ;;
                esac
            ) && return 0 || return 1
        done
        return 1
    }

    # Keep unused dependency placeholders valid YAML after envsubst.
    export DEB_LIB_DEPENDS="      # unused for {{ format }}"
    export RPM_LIB_DEPENDS="      # unused for {{ format }}"
    export RPM_GSTREAMER_BASE="gstreamer1-plugins-base"
    export RPM_GSTREAMER_GOOD="gstreamer1-plugins-good"
    export RPM_GSTREAMER_PIPEWIRE="pipewire-gstreamer"
    if [ "{{ format }}" = rpm ] && is_suse; then
        export RPM_GSTREAMER_BASE="gstreamer-plugins-base"
        export RPM_GSTREAMER_GOOD="gstreamer-plugins-good"
        export RPM_GSTREAMER_PIPEWIRE="gstreamer-plugin-pipewire"
    fi
    case "{{ format }}" in
    deb) export DEB_LIB_DEPENDS="$lib_depends" ;;
    rpm) export RPM_LIB_DEPENDS="$lib_depends" ;;
    esac

    tmp_config=$(mktemp)
    envsubst '$MULTIARCH $OPENCV_MIN $OPENCV_NEXT $DEB_LIB_DEPENDS $RPM_LIB_DEPENDS $RPM_GSTREAMER_BASE $RPM_GSTREAMER_GOOD $RPM_GSTREAMER_PIPEWIRE' < {{ quote(config) }} > "$tmp_config"
    {{ quote(nfpm) }} pkg -f "$tmp_config" --packager {{ format }} --target dist/packages
    rm -f "$tmp_config"

[private]
_dist-packages:
    mkdir -p dist/packages

# Assert every packaged format pins the opencv soversion gazed linked against, so a bump fails the
# transaction rather than crash-looping; that every package ships its payload, maintainer scriptlets
# and upgrade-preserved config files; and that no two packages claim the same path.
[arg("format", pattern="deb|rpm|archlinux")]
[private]
_verify-package format:
    #!/usr/bin/env bash
    set -euo pipefail

    fail() { echo "verify: FAIL: $*" >&2; exit 1; }
    ok() { echo "verify: $* ✔"; }

    newest() { ls -t $1 2>/dev/null | head -n1 || true; }

    locate() {
        case "{{ format }}" in
        deb) newest "dist/packages/$1_[0-9]*.deb" ;;
        rpm) newest "dist/packages/$1-[0-9]*.rpm" ;;
        archlinux) newest "dist/packages/$1-[0-9]*.pkg.tar.*" ;;
        esac
    }

    manifest() {
        case "{{ format }}" in
        deb) dpkg-deb -c "$1" | awk '{ print $1 "\t" $3 "\t" $6 }' ;;
        rpm) rpm -qp --queryformat '[%{FILEMODES:perms}\t%{FILESIZES}\t%{FILENAMES}\n]' "$1" 2>/dev/null ;;
        archlinux) tar -tvf "$1" | awk '{ print $1 "\t" $3 "\t" $6 }' ;;
        esac | awk -F'\t' -v OFS='\t' '
            { path = $3
              sub(/^\.\//, "/", path)
              if (path !~ /^\//) path = "/" path
              sub(/\/+$/, "", path)
              if (path == "" || path ~ /^\/\./) next
              type = substr($1, 1, 1)
              print path, (type == "d" ? "d" : (type == "l" ? "l" : "f")), $2 }'
    }

    scriptlets() {
        case "{{ format }}" in
        deb) dpkg-deb --ctrl-tarfile "$1" | tar -t | sed 's#^\./##; /^$/d' ;;
        rpm) rpm -qp --scripts "$1" 2>/dev/null | sed -n 's/^\([a-z]*\) scriptlet.*/\1/p' ;;
        archlinux)
            local scriptlet status
            scriptlet=$(tar -xOf "$1" .INSTALL 2>&1) && status=0 || status=$?
            [ "$status" -eq 0 ] || fail "cannot read .INSTALL from $(basename "$1"): $scriptlet"
            # nfpm >= 2.47 wraps each hook as `function post_install() {`; older
            # releases emitted a bare `post_install() {`. Accept both.
            sed -nE 's/^(function[[:space:]]+)?([a-z_]+)[[:space:]]*\(\).*/\2/p' <<< "$scriptlet"
            ;;
        esac
    }

    config_files() {
        case "{{ format }}" in
        deb) dpkg-deb -I "$1" conffiles 2>/dev/null | awk 'NF { print $1 }' || true ;;
        rpm) rpm -qp --queryformat '[%{FILENAMES}\t%{FILEFLAGS:fflags}\n]' "$1" 2>/dev/null | awk -F'\t' '$2 ~ /c/ && $2 ~ /n/ { print $1 }' ;;
        archlinux) tar -xOf "$1" .PKGINFO | sed -n 's#^backup = #/#p' ;;
        esac
    }

    depends_of() {
        case "{{ format }}" in
        deb) dpkg-deb -f "$1" Depends | tr ',' '\n' | sed -E 's/^[[:space:]]+//; s/[[:space:]]*\(.*$//; /^$/d' ;;
        rpm) rpm -qp --requires "$1" 2>/dev/null ;;
        archlinux) tar -xOf "$1" .PKGINFO | sed -n 's/^depend = //p' ;;
        esac
    }

    pkg_version() {
        case "{{ format }}" in
        deb) dpkg-deb -f "$1" Version ;;
        rpm) rpm -qp --queryformat '%{VERSION}-%{RELEASE}' "$1" 2>/dev/null ;;
        archlinux) tar -xOf "$1" .PKGINFO | sed -n 's/^pkgver = //p' ;;
        esac
    }

    pkg_arch() {
        case "{{ format }}" in
        deb) dpkg-deb -f "$1" Architecture ;;
        rpm) rpm -qp --queryformat '%{ARCH}' "$1" 2>/dev/null ;;
        archlinux) tar -xOf "$1" .PKGINFO | sed -n 's/^arch = //p' ;;
        esac
    }

    owned=$(mktemp)
    trap 'rm -f "$owned"' EXIT
    versions=()
    arches=()

    load() {
        pkg=$(locate "$1")
        [ -n "$pkg" ] || fail "no {{ format }} $1 package in dist/packages"
        base=$(basename "$pkg")
        rows=$(manifest "$pkg")
        paths=$(cut -f1 <<< "$rows")
        depends=$(depends_of "$pkg")
        versions+=("$(pkg_version "$pkg")")
        arches+=("$(pkg_arch "$pkg")")
        [ -n "$paths" ] || fail "$base ships no files"
        awk -F'\t' -v n="$base" '$2 != "d" { print $1 "\t" n }' <<< "$rows" >> "$owned"
        if grep -Eq -- '-(dev|devel)$' <<< "$depends"; then
            fail "$base depends on a development package: $(tr '\n' ' ' <<< "$depends")"
        fi
    }

    want_files() {
        local missing=() p
        for p in "$@"; do grep -Fxq "$p" <<< "$paths" || missing+=("$p"); done
        [ ${#missing[@]} -eq 0 ] || fail "$base does not ship: ${missing[*]}"
    }

    want_size() {
        local size
        size=$(awk -F'\t' -v p="$1" '$1 == p && $2 == "f" { print $3; exit }' <<< "$rows")
        [ -n "$size" ] || fail "$base does not ship $1 as a regular file"
        [ "$size" -ge "$2" ] || fail "$base ships a ${size}-byte $1; expected at least $2 bytes"
    }

    want_scripts() {
        local have missing=() s
        have=$(scriptlets "$pkg")
        for s in "$@"; do grep -Fxq "$s" <<< "$have" || missing+=("$s"); done
        [ ${#missing[@]} -eq 0 ] || fail "$base lacks maintainer scriptlets: ${missing[*]}; upgrades will skip them"
    }

    want_config() {
        local have missing=() p
        have=$(config_files "$pkg")
        for p in "$@"; do grep -Fxq "$p" <<< "$have" || missing+=("$p"); done
        [ ${#missing[@]} -eq 0 ] || fail "$base does not preserve local edits to: ${missing[*]}"
    }

    want_depends() {
        local missing=() d
        for d in "$@"; do grep -Fxq "$d" <<< "$depends" || missing+=("$d"); done
        [ ${#missing[@]} -eq 0 ] || fail "$base lacks runtime dependencies: ${missing[*]}; has $(tr '\n' ' ' <<< "$depends")"
    }

    want_opencv_bounds() {
        if grep -Eq '^opencv>=[0-9]+\.[0-9]+$' <<< "$depends" && grep -Eq '^opencv<[0-9]+\.[0-9]+$' <<< "$depends"; then
            ok "$base pins opencv ($(grep -E '^opencv[<>=]' <<< "$depends" | tr '\n' ' '))"
        else
            fail "$base lacks a version-bounded opencv dependency; an opencv soname bump will crash-loop it"
        fi
    }

    is_suse() {
        local os_rel=""
        for os_rel in /etc/os-release /usr/lib/os-release; do
            [ -r "$os_rel" ] || continue
            (
                # shellcheck disable=SC1091
                . "$os_rel"
                case "${ID:-} ${ID_LIKE:-}" in
                *opensuse*|*suse*) exit 0 ;;
                *) exit 1 ;;
                esac
            ) && return 0 || return 1
        done
        return 1
    }

    suse=""
    if [ "{{ format }}" = rpm ] && is_suse; then
        suse=1
    fi

    case "{{ format }}" in
    deb)
        post=postinst; preun=prerm; postun=postrm
        gst=(gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire)
        ;;
    rpm)
        post=postinstall; preun=preuninstall; postun=postuninstall
        if [ -n "$suse" ]; then
            gst=(gstreamer-plugins-base gstreamer-plugins-good gstreamer-plugin-pipewire)
        else
            gst=(gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer)
        fi
        ;;
    archlinux)
        post=post_install; preun=pre_remove; postun=post_remove
        gst=(gst-plugins-base gst-plugins-good gst-plugin-pipewire)
        ;;
    esac

    load gaze
    want_files /usr/bin/gazed /usr/bin/gaze /etc/gaze/config.toml \
        /etc/dbus-1/system.d/com.gundulabs.Gaze.conf \
        /usr/share/polkit-1/actions/com.gundulabs.gaze.policy \
        /usr/lib/systemd/system/gazed.service
    want_size /usr/bin/gazed 100000
    want_size /usr/bin/gaze 100000
    want_config /etc/gaze/config.toml /etc/dbus-1/system.d/com.gundulabs.Gaze.conf
    want_scripts "$post" "$preun"
    want_depends "${gst[@]}"
    case "{{ format }}" in
    deb)
        want_files "/lib/{{ multiarch }}/security/pam_gaze.so" \
            "/lib/{{ multiarch }}/security/pam_gaze_grosshack.so" \
            /usr/lib/security/pam_gaze.so /usr/lib/security/pam_gaze_grosshack.so \
            /usr/share/pam-configs/gaze /usr/share/pam-configs/gaze-simultaneous
        want_size "/lib/{{ multiarch }}/security/pam_gaze.so" 20000
        want_size "/lib/{{ multiarch }}/security/pam_gaze_grosshack.so" 20000
        for module in pam_gaze pam_gaze_grosshack; do
            target=$(dpkg-deb -c "$pkg" | sed -n "s#.* \./usr/lib/security/${module}\.so -> ##p")
            [ "$target" = "/lib/{{ multiarch }}/security/${module}.so" ] || {
                echo "verify: FAIL: $base points /usr/lib/security/${module}.so at '${target}'," >&2
                fail "expected /lib/{{ multiarch }}/security/${module}.so; libpam will not find the module"
            }
        done
        want_depends libpam0g
        if grep -Eq 'libopencv-core[0-9]' <<< "$depends"; then
            ok "$base pins opencv ($(grep -oE 'libopencv-[a-z]+[0-9]+[a-z0-9]*' <<< "$depends" | tr '\n' ' '))"
        else
            fail "$base lacks a soversioned opencv dependency: $(tr '\n' ' ' <<< "$depends")"
        fi
        ;;
    rpm)
        want_files /usr/lib64/security/pam_gaze.so /usr/lib64/security/pam_gaze_grosshack.so \
            /usr/share/gaze/gaze-gdm-camera.pp
        want_size /usr/lib64/security/pam_gaze.so 20000
        want_size /usr/lib64/security/pam_gaze_grosshack.so 20000
        if [ -n "$suse" ]; then
            want_files /usr/lib/pam-config.d/gaze.conf
            want_depends pam pam-config
        else
            want_files /usr/share/authselect/vendor/gaze/system-auth \
                /usr/share/authselect/vendor/gaze/password-auth \
                /usr/share/authselect/vendor/gaze/fingerprint-auth
            want_depends pam
        fi
        if grep -Eq 'libopencv_core\.so\.[0-9]+' <<< "$depends"; then
            ok "$base pins opencv ($(grep -oE 'libopencv_[a-z0-9]+\.so\.[0-9]+' <<< "$depends" | tr '\n' ' '))"
        else
            fail "$base lacks a soname opencv requirement"
        fi
        ;;
    archlinux)
        want_files /usr/lib/security/pam_gaze.so /usr/lib/security/pam_gaze_grosshack.so
        want_size /usr/lib/security/pam_gaze.so 20000
        want_size /usr/lib/security/pam_gaze_grosshack.so 20000
        want_scripts post_upgrade
        want_depends pam tpm2-tss
        want_opencv_bounds
        ;;
    esac
    ok "$base ships its daemon, PAM modules, polkit action, unit and config"

    load gaze-gui
    want_files /usr/bin/gaze-gui /usr/share/applications/com.gundulabs.Gaze.desktop \
        /usr/share/icons/hicolor/scalable/apps/com.gundulabs.Gaze.svg \
        /usr/share/metainfo/com.gundulabs.Gaze.metainfo.xml
    want_size /usr/bin/gaze-gui 100000
    want_depends "${gst[@]}"
    if [ "{{ format }}" = archlinux ]; then
        want_depends gtk4 libadwaita
        want_opencv_bounds
    fi
    ok "$base ships its binary, desktop entry, icon and metainfo"

    load gaze-gnome-extension
    want_files /usr/share/gnome-shell/extensions/gaze@gundulabs.com/metadata.json \
        /usr/share/gnome-shell/extensions/gaze@gundulabs.com/extension.js \
        /usr/share/gnome-shell/extensions/gaze@gundulabs.com/prefs.js \
        /usr/share/glib-2.0/schemas/org.gnome.shell.extensions.gaze.gschema.xml \
        /etc/dconf/db/gdm.d/00-gaze-defaults /usr/share/gaze/dconf-profile-gdm \
        /etc/pam.d/gdm-face
    want_config /etc/dconf/db/gdm.d/00-gaze-defaults /etc/pam.d/gdm-face
    want_scripts "$post" "$postun"
    want_depends gnome-shell
    if [ "{{ format }}" = archlinux ]; then
        want_scripts post_upgrade
    fi
    ok "$base ships the extension, gschema, gdm defaults and gdm-face stack"

    load gaze-cinnamon-extension
    want_files /usr/share/cinnamon/extensions/gaze@gundulabs.com/metadata.json \
        /usr/share/cinnamon/extensions/gaze@gundulabs.com/extension.js \
        /usr/share/cinnamon/extensions/gaze@gundulabs.com/settings-schema.json
    want_depends cinnamon
    ok "$base ships the Cinnamon extension files"

    load gaze-hyprlock
    want_files /etc/pam.d/hyprlock-gaze /etc/pam.d/hyprlock-gaze-simultaneous
    want_config /etc/pam.d/hyprlock-gaze /etc/pam.d/hyprlock-gaze-simultaneous
    want_scripts "$post" "$postun"
    if [ "{{ format }}" = archlinux ]; then
        want_scripts post_upgrade
    fi
    ok "$base ships both hyprlock PAM stacks"

    load gaze-kde
    want_files /usr/bin/gaze-kde-pam \
        /usr/share/plasma/systemsettings/externalmodules/gaze-face-unlock.desktop
    want_size /usr/bin/gaze-kde-pam 1000
    want_scripts "$post" "$postun"
    if [ "{{ format }}" = archlinux ]; then
        want_scripts post_upgrade
    fi
    if grep -Fxq /etc/pam.d/kde-fingerprint <<< "$paths"; then
        fail "$base ships /etc/pam.d/kde-fingerprint, which plasma-workspace owns; the transaction will conflict"
    fi
    ok "$base ships the PAM helper and System Settings entry, and claims no plasma-owned file"

    unique_versions=$(printf '%s\n' "${versions[@]}" | sort -u)
    [ "$(wc -l <<< "$unique_versions")" -eq 1 ] \
        || fail "packages disagree on version: $(tr '\n' ' ' <<< "$unique_versions"); dist/packages holds stale artifacts"
    unique_arches=$(printf '%s\n' "${arches[@]}" | sort -u)
    [ "$(wc -l <<< "$unique_arches")" -eq 1 ] \
        || fail "packages disagree on architecture: $(tr '\n' ' ' <<< "$unique_arches")"
    overlap=$(cut -f1 "$owned" | sort | uniq -d)
    [ -z "$overlap" ] \
        || fail "more than one package ships: $(tr '\n' ' ' <<< "$overlap")"
    ok "all ${#versions[@]} packages are $unique_versions ($unique_arches) and claim disjoint paths"

# Build nfpm packages for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
[parallel]
package format: build-rust build-selinux && (package-prebuilt format)

# Package already-built artifacts for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
package-prebuilt format: _dist-packages
    #!/usr/bin/env bash
    set -euo pipefail

    is_suse() {
        local os_rel=""
        for os_rel in /etc/os-release /usr/lib/os-release; do
            [ -r "$os_rel" ] || continue
            (
                # shellcheck disable=SC1091
                . "$os_rel"
                case "${ID:-} ${ID_LIKE:-}" in
                *opensuse*|*suse*) exit 0 ;;
                *) exit 1 ;;
                esac
            ) && return 0 || return 1
        done
        return 1
    }

    # Use SUSE manifests together so packages do not mix PAM stack formats.
    # Other RPM hosts keep the existing manifests.

    configs=(packaging/nfpm.yaml packaging/nfpm-gui.yaml packaging/nfpm-gnome-extension.yaml packaging/nfpm-cinnamon-extension.yaml packaging/nfpm-hyprlock.yaml packaging/nfpm-kde.yaml)
    if [ "{{ format }}" = rpm ] && is_suse; then
        configs=(packaging/nfpm-opensuse.yaml packaging/nfpm-gui.yaml packaging/nfpm-gnome-extension-opensuse.yaml packaging/nfpm-cinnamon-extension.yaml packaging/nfpm-hyprlock-opensuse.yaml packaging/nfpm-kde.yaml)
    fi

    for config in "${configs[@]}"; do {{ quote(just_executable()) }} _nfpm "$config" "{{ format }}"; done
    {{ quote(just_executable()) }} _verify-package "{{ format }}"
    echo "Packages written to dist/packages/"

# ── srpm ──────────────────────────────────────────────────────────────────────

srpm_topdir := env("SRPM_TOPDIR", "dist/srpm")

# Collect everything the spec needs as a Source, so Copr (and any other mock
# builder) can build with the network disabled.
[private]
_srpm-sources:
    #!/usr/bin/env bash
    set -euo pipefail

    sources="{{ srpm_topdir }}/SOURCES"
    mkdir -p "$sources"

    git archive --format=tar.gz --prefix="gaze-{{ version }}/" \
        -o "$sources/gaze-{{ version }}.tar.gz" HEAD

    cargo vendor --locked --versioned-dirs > "$sources/cargo-vendor-config.toml"
    tar --zstd -cf "$sources/vendor.tar.zst" vendor

    for ort_arch in x64 aarch64; do
        ort_file="onnxruntime-linux-${ort_arch}-{{ ort_version }}.tgz"
        [ -s "$sources/$ort_file" ] && continue
        curl -fsSL \
            "https://github.com/microsoft/onnxruntime/releases/download/v{{ ort_version }}/${ort_file}" \
            -o "$sources/$ort_file"
    done

# Build a source RPM (Copr input). Set RPM_SIGN_KEY to a gpg key id to sign it.
[group("package")]
srpm: _dist-packages _srpm-sources
    #!/usr/bin/env bash
    set -euo pipefail

    export VERSION="{{ version }}"
    export PACKAGE_RELEASE="{{ package_release }}"
    export ORT_VERSION="{{ ort_version }}"
    export CHANGELOG_DATE="$(date -u '+%a %b %d %Y')"
    export SCRIPTLET_MAIN_POST="$(cat packaging/postinst-rpm.sh)"
    export SCRIPTLET_EXTENSION_POST="$(cat packaging/postinst-gnome-extension.sh)"
    export SCRIPTLET_EXTENSION_POSTUN="$(cat packaging/postrm-gnome-extension.sh)"
    export SCRIPTLET_KDE_POST="$(cat packaging/postinst-kde.sh)"
    export SCRIPTLET_KDE_POSTUN="$(cat packaging/postrm-kde.sh)"
    export SCRIPTLET_HYPRLOCK_POST="$(cat packaging/postinst-hyprlock.sh)"
    export SCRIPTLET_HYPRLOCK_POSTUN="$(cat packaging/postrm-hyprlock.sh)"

    mkdir -p "{{ srpm_topdir }}/SPECS"
    spec="{{ srpm_topdir }}/SPECS/gaze.spec"
    envsubst '$VERSION $PACKAGE_RELEASE $ORT_VERSION $CHANGELOG_DATE $SCRIPTLET_MAIN_POST $SCRIPTLET_EXTENSION_POST $SCRIPTLET_EXTENSION_POSTUN $SCRIPTLET_HYPRLOCK_POST $SCRIPTLET_HYPRLOCK_POSTUN $SCRIPTLET_KDE_POST $SCRIPTLET_KDE_POSTUN' \
        < packaging/rpm/gaze.spec.in > "$spec"

    {{ quote(rpmbuild) }} -bs "$spec" --define "_topdir $PWD/{{ srpm_topdir }}"

    srpm=$(ls -t "{{ srpm_topdir }}"/SRPMS/gaze-*.src.rpm 2>/dev/null | head -n1 || true)
    [ -n "$srpm" ] || { echo "srpm: rpmbuild produced no source package" >&2; exit 1; }

    if [ -n "${RPM_SIGN_KEY:-}" ]; then
        rpmsign --define "_gpg_name ${RPM_SIGN_KEY}" --addsign "$srpm"
        checksig=$(rpm -Kv "$srpm" 2>&1 || true)
        grep -qiE "${RPM_SIGN_KEY: -16}|signature" <<< "$checksig" \
            || { printf '%s\n' "$checksig" >&2; echo "srpm: FAIL: signing did not attach a signature to $srpm" >&2; exit 1; }
        echo "verify: $(basename "$srpm") signed by ${RPM_SIGN_KEY} ✔"
    fi

    contents=$(rpm -qpl "$srpm")
    for required in vendor.tar.zst "onnxruntime-linux-x64-{{ ort_version }}.tgz" "onnxruntime-linux-aarch64-{{ ort_version }}.tgz"; do
        grep -qxF "$required" <<< "$contents" \
            || { echo "srpm: FAIL: $(basename "$srpm") is missing $required; mock builds have no network" >&2; exit 1; }
    done
    echo "verify: $(basename "$srpm") bundles vendored crates and ONNX Runtime {{ ort_version }} ✔"

    cp -f "$srpm" dist/packages/
    cp -f packaging/rpm/copr-target.env dist/packages/copr-target.env
    echo "Source RPM written to dist/packages/$(basename "$srpm")"

# Remove all generated artifacts
[group("dev")]
clean:
    cargo clean
    rm -rf dist
    rm -rf flatpak-build .flatpak-builder
    rm -rf .flatpak-cache
    rm -rf vendor

# ── dev helpers ───────────────────────────────────────────────────────────────

# Enable Git hooks for this clone
[group("dev")]
setup-hooks:
    scripts/setup-hooks.sh

# Run the full test suite
[group("checks")]
test:
    @{{ gui_notice }}
    {{ opencv_env }} cargo test --workspace {{ gui_exclude }} --release
    {{ opencv_env }} cargo test -p gaze-core --release --no-default-features --features gaze-core/openvino-config config::

# Run the OpenVINO-gated tests with an OpenVINO-enabled system ONNX Runtime.
[group("checks")]
test-openvino:
    {{ opencv_env }} cargo test -p gaze-core --release --features gaze-core/openvino -- inference:: config::

# Check dependencies for known security advisories
[group("checks")]
audit:
    cargo audit

# Run clippy lints across the workspace
[group("checks")]
lint:
    @{{ gui_notice }}
    {{ opencv_env }} cargo clippy --workspace {{ gui_exclude }} --all-targets -- -D warnings

# Lint the OpenVINO-gated code with an OpenVINO-enabled system ONNX Runtime.
[group("checks")]
lint-openvino:
    {{ opencv_env }} cargo clippy -p gaze-core --all-targets --features gaze-core/openvino -- -D warnings

# Check formatting (does not write)
[group("checks")]
fmt-check:
    cargo fmt --all -- --check

# Apply formatting
[group("dev")]
fmt:
    cargo fmt --all

# Also enables TPM template encryption when a TPM is present; GAZE_DEV_TPM=0 skips it.
# Link the installed system runtime to this checkout's release build
[group("dev")]
dev-link-system: build-rust
    sudo GAZE_DEV_TPM="${GAZE_DEV_TPM:-1}" scripts/dev-link-system.sh enable

# Restore package-installed files that dev-link-system replaced
[group("dev")]
dev-unlink-system:
    sudo scripts/dev-link-system.sh disable

# Drive a PAM service the way KScreenLocker's greeter does; fails on a prompt.
# With confdir=self, builds a throwaway stack around this checkout's module so a
# slot can be driven without editing the one the real lock screen uses.
[group("dev")]
kde-harness service="kde-fingerprint" rounds="1" confdir="":
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p dist
    cc -Wall -Wextra -O2 -o dist/kde-pam-harness scripts/kde-pam-harness.c -lpam
    echo "built dist/kde-pam-harness"
    confdir='{{ confdir }}'
    if [ "$confdir" = self ]; then
        cargo build --release -p pam-gaze
        # A space-free directory, because a PAM config field cannot quote a path.
        confdir=$(mktemp -d /tmp/gaze-kde-harness.XXXXXX)
        trap 'rm -rf "$confdir"' EXIT
        cp target/release/libpam_gaze.so "$confdir/pam_gaze_test.so"
        printf '#%%PAM-1.0\nauth [success=done default=ignore] %s/pam_gaze_test.so\nauth required pam_deny.so\naccount required pam_permit.so\n' \
            "$confdir" >"$confdir/{{ service }}"
    fi
    # Unprivileged, like kscreenlocker_greet; needs gazed running and a face enrolled.
    if [ -n "$confdir" ]; then
        ./dist/kde-pam-harness '{{ service }}' "$USER" '{{ rounds }}' "$confdir"
    else
        ./dist/kde-pam-harness '{{ service }}' "$USER" '{{ rounds }}'
    fi

# Show which installed Gaze paths are linked to this checkout
[group("dev")]
dev-link-status:
    scripts/dev-link-system.sh status

# Build docs
[group("docs")]
build-docs:
    bun install
    bun run docs:build

# ── docker (build the Linux targets on a non-Linux host) ────────────────────────

# Tag for the local Linux build-environment image
docker_image := env("GAZE_DOCKER_IMAGE", "gaze-build:local")

# Build (or refresh) the Linux build-environment image; cached after the first run
[group("docker")]
docker-image:
    docker build -t {{ quote(docker_image) }} -f packaging/docker/Dockerfile.build packaging/docker

# flatpak-builder writes ostree, which a sshfs-backed /work bind mount can't host, so its dirs go
# to the in-VM /flatpak volume; the bundle itself is a plain file and still lands in dist/packages.
# Run any build/package target in the Linux container, e.g. `just docker build-rust`
[group("docker")]
docker target *args: docker-image
    docker run --rm --privileged \
        -v {{ quote(justfile_directory() + ":/work") }} \
        -v gaze-cargo-registry:/root/.cargo/registry \
        -v gaze-cargo-git:/root/.cargo/git \
        -v gaze-target:/work/target \
        -v gaze-flatpak:/root/.local/share/flatpak \
        -v gaze-flatpak-work:/flatpak \
        -e FLATPAK_STATE_DIR=/flatpak/state \
        -e FLATPAK_BUILD_DIR=/flatpak/build \
        -e FLATPAK_REPO_DIR=/flatpak/repo \
        -e CARGO_BUILD_JOBS -e FLATPAK_BUILDER_JOBS -e FLATPAK_GPG_SIGN \
        -e VERSION -e PACKAGE_RELEASE \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        {{ quote(docker_image) }} \
        {{ target }} {{ args }}
