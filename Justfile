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
ort_version := env("ORT_VERSION", "1.32.0")
# Intel's onnxruntime-openvino build, which trails `ort_version`. Only `test-openvino` uses it.
ort_openvino_version := env("ORT_OPENVINO_VERSION", "1.24.1")
ort_openvino_dir := justfile_directory() / "target/ort-openvino"

# The opencv crate probes only the `opencv4`/`opencv` pkg-config names, so distros shipping
# OpenCV 5 (e.g. Arch) need this override. Empty when opencv4/opencv resolve or opencv5 doesn't.
opencv_system_env := shell("pkg-config --exists opencv4 2>/dev/null || pkg-config --exists opencv 2>/dev/null || ! pkg-config --exists opencv5 2>/dev/null || echo OPENCV_PKGCONFIG_NAME=opencv5")

# `GAZE_OPENCV=static` links a pinned OpenCV into the binaries, so one Arch package also installs on
# derivatives whose repositories carry a different OpenCV.
opencv := lowercase(env("GAZE_OPENCV", "system"))
opencv_version := env("OPENCV_VERSION", "4.14.0")
opencv_recipe := replace_regex(sha256_file("scripts/build-opencv.sh"), '^(.{12}).*$', '$1')
opencv_dir := env("OPENCV_CACHE_DIR", justfile_directory() / "target/opencv-static") / opencv_version + "-" + arch + "-" + opencv_recipe
opencv_static_env := "OPENCV_DISABLE_PROBES=pkg_config,cmake,vcpkg_cmake,vcpkg OPENCV_LINK_LIBS=static=opencv_imgproc,static=opencv_core,dylib=z OPENCV_LINK_PATHS=" + quote(opencv_dir / "lib") + " OPENCV_INCLUDE_PATHS=" + quote(opencv_dir / "include/opencv4")
opencv_env := if opencv == "static" { opencv_static_env } else if opencv == "system" { opencv_system_env } else { error("GAZE_OPENCV must be system or static, got " + opencv) }

# Build the GTK front-end (`gaze-gui`). `GAZE_GUI=0`/`false`/`no`/`off` drops it,
# and gtk4/libadwaita, from the build, test, and lint recipes below only.
gui := lowercase(env("GAZE_GUI", "1"))
gui_off := if gui =~ '^(0|false|no|off)$' { "1" } else { "" }
gui_pkg := if gui_off == "1" { "" } else { "-p gaze-gui" }
gui_exclude := if gui_off == "1" { "--exclude gaze-gui" } else { "" }
gui_notice := if gui_off == "1" { "echo 'note: GAZE_GUI is off, gaze-gui is excluded here; CI still checks it'" } else { "true" }

# `GAZE_KCM=legacy` stages the System Settings entry that opens gaze-gui instead of the Plasma 6 page,
# for hosts without KDE Frameworks 6 such as Ubuntu 24.04, which still ships Plasma 5.
kcm := lowercase(env("GAZE_KCM", "plasma6"))

# Derived vars
multiarch := if arch == "aarch64" { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" }
deb_arch := if arch == "x86_64" { "amd64" } else if arch == "aarch64" { "arm64" } else { arch }

# List recipes when `just` is run without arguments.
[default]
[private]
default:
    @{{ quote(just_executable()) }} --justfile {{ quote(justfile()) }} --list

# ── build ─────────────────────────────────────────────────────────────────────

# Two invocations keep gaze-vision's detector/inference code out of the client binaries.
# Only the daemon loads ONNX Runtime, after checking CPU support.
# Build all Rust workspace binaries (release)
[group("build")]
build-rust: prepare-ort prepare-opencv && _stage-opencv
    {{ opencv_env }} cargo build --locked -p gazed --release
    {{ opencv_env }} cargo build --locked -p gaze-cli {{ gui_pkg }} -p pam-gaze --release
    scripts/check-pam-link.sh target/release/libpam_gaze.so

# Stage the gaze-kde System Settings files in dist/kcm
[arg("format", pattern="deb|rpm|archlinux")]
[group("build")]
build-kcm format="rpm":
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf dist/kcm
    case {{ quote(kcm) }} in
    plasma6)
        cmake -S integrations/kde/kcm -B target/kcm -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_BUILD_TYPE=Release
        cmake --build target/kcm
        DESTDIR="$PWD/dist/kcm" cmake --install target/kcm
        ;;
    legacy)
        [ "{{ format }}" != rpm ] || { echo "build-kcm: every RPM target runs Plasma 6, so GAZE_KCM=legacy is for deb and Arch only" >&2; exit 1; }
        install -Dm0644 integrations/kde/gaze-face-unlock.desktop \
            dist/kcm/usr/share/plasma/systemsettings/externalmodules/gaze-face-unlock.desktop
        ;;
    *)
        echo "build-kcm: GAZE_KCM must be plasma6 or legacy, got {{ kcm }}" >&2
        exit 1
        ;;
    esac

# Compile the GNOME and Cinnamon extension translations into dist/locale
[group("build")]
build-locales:
    rm -rf dist/locale
    scripts/build-locales.sh integrations/gnome-shell/po dist/locale/gnome/usr/share/gnome-shell/extensions/gaze@gundulabs.com/locale
    scripts/build-locales.sh integrations/cinnamon/po dist/locale/cinnamon/usr/share/locale

[private]
prepare-ort:
    bash scripts/prepare-ort.sh {{ quote(ort_version) }} {{ quote(arch) }}

[private]
prepare-opencv:
    @{{ if opencv == "static" { "bash scripts/build-opencv.sh " + quote(opencv_version) + " " + quote(opencv_dir) } else { "true" } }}

[private]
_stage-opencv:
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf dist/opencv
    mkdir -p dist/opencv
    [ {{ quote(opencv) }} = static ] || exit 0
    binaries=(target/release/gazed target/release/gaze)
    [ -n {{ quote(gui_off) }} ] || binaries+=(target/release/gaze-gui)
    if objdump -p "${binaries[@]}" | grep 'NEEDED.*libopencv_'; then
        echo "build-rust: GAZE_OPENCV=static, but the binaries above still link a shared OpenCV" >&2
        exit 1
    fi
    install -m0644 {{ quote(opencv_dir / "LICENSE") }} {{ quote(opencv_dir / "SoftFloat-COPYING.txt") }} dist/opencv/

[private]
prepare-ort-openvino:
    bash scripts/prepare-ort-openvino.sh {{ quote(ort_openvino_version) }} {{ quote(ort_openvino_dir / "lib") }}

# Compile the SELinux policy module
[group("build")]
build-selinux:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p dist/selinux
    if command -v checkmodule >/dev/null 2>&1 && command -v semodule_package >/dev/null 2>&1; then
        for module in gaze-gdm-camera gaze-greeter-keyring; do
            checkmodule -M -m -o "dist/selinux/$module.mod" "packaging/selinux/$module.te"
            semodule_package -o "dist/selinux/$module.pp" -m "dist/selinux/$module.mod"
            rm -f "dist/selinux/$module.mod"
            echo "Built dist/selinux/$module.pp"
        done
    else
        echo "WARNING: SELinux tools not found. Skipping SELinux policy build." >&2
    fi

[private]
prepare-flatpak-vendor:
    mkdir -p .flatpak-cache/cargo
    cargo vendor --locked --versioned-dirs > .flatpak-cache/cargo/config.toml

# flatpak-builder's ostree dirs need xattrs and same-filesystem co-location, so they default to
# the repo tree; the `docker` wrapper moves them into a volume the sshfs mount can't host.
flatpak_state_dir := env("FLATPAK_STATE_DIR", ".flatpak-builder")
flatpak_build_dir := env("FLATPAK_BUILD_DIR", "flatpak-build")
flatpak_repo_dir := env("FLATPAK_REPO_DIR", "dist/flatpak-repo")

# Build flatpak repo and bundle
[group("build")]
build-flatpak: prepare-flatpak-vendor
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
    done < <(grep -oE 'target/release/(gaze-gui|gazed|gaze|lib[A-Za-z0-9_.+-]+)' {{ quote(config) }} | sort -u)
    # build-kcm stages either the Plasma 6 page or the legacy entry, with paths that vary by distro.
    export KCM_CONTENTS="  # unused for {{ config }}"
    export KCM_DEPENDS="      # unused for {{ config }}"
    export KCM_RECOMMENDS="      # unused for {{ config }}"
    if grep -q 'KCM_CONTENTS' {{ quote(config) }}; then
        [ -d dist/kcm ] || { echo "_nfpm: {{ config }} ships dist/kcm, which is not staged; run just build-kcm {{ format }}" >&2; exit 1; }
        KCM_CONTENTS=""
        native=""
        while read -r src; do
            mode=0644
            [ -x "$src" ] && mode=0755
            printf -v entry '  - src: "%s"\n    dst: "%s"\n    file_info:\n      mode: %s\n' "$src" "${src#dist/kcm}" "$mode"
            KCM_CONTENTS+=$entry
            case "$src" in *.so) binaries+=("$src"); native=1 ;; esac
        done < <(find dist/kcm -type f | sort)
        [ -n "$KCM_CONTENTS" ] || { echo "_nfpm: dist/kcm is empty; run just build-kcm {{ format }}" >&2; exit 1; }
        [ -n "$native" ] || [ "{{ format }}" != rpm ] || { echo "_nfpm: RPM packages must ship the Plasma 6 page" >&2; exit 1; }
        if [ -n "$native" ]; then
            case "{{ format }}" in
            deb) KCM_DEPENDS=$(printf '      - %s\n' qml6-module-org-kde-kirigami qml6-module-org-kde-kcmutils) ;;
            rpm) KCM_DEPENDS=$(printf '      - %s\n' kf6-kirigami) ;;
            archlinux) KCM_DEPENDS=$(printf '      - %s\n' kcmutils kirigami) ;;
            esac
        else
            KCM_RECOMMENDS="      - gaze-gui"
        fi
    fi

    # build-locales stages one catalog per language.
    export LOCALE_CONTENTS="  # unused for {{ config }}"
    if grep -q 'LOCALE_CONTENTS' {{ quote(config) }}; then
        case {{ quote(config) }} in
        *cinnamon*) locales=dist/locale/cinnamon ;;
        *) locales=dist/locale/gnome ;;
        esac
        [ -d "$locales" ] || { echo "_nfpm: {{ config }} ships $locales, which is not staged; run just build-locales" >&2; exit 1; }
        LOCALE_CONTENTS=""
        while read -r src; do
            printf -v entry '  - src: "%s"\n    dst: "%s"\n    file_info:\n      mode: 0644\n' "$src" "${src#"$locales"}"
            LOCALE_CONTENTS+=$entry
        done < <(find "$locales" -type f | sort)
    fi

    needed() { objdump -p "${binaries[@]}" | awk '/NEEDED/ { print $2 }' | sort -u; }

    for module in "${binaries[@]}"; do
        case "$module" in
        */libpam_gaze.so) scripts/check-pam-link.sh "$module" ;;
        esac
    done
    export OPENCV_CONTENTS="  # unused for {{ config }}"
    export OPENCV_DEPENDS="      # unused for {{ format }}"
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
            # Arch bumps the OpenCV soname every minor release, so pin the soversion of a shared
            # link; unpinned, an upgrade crash-loops the daemon instead of failing.
            opencv_soname=$(needed | grep '^libopencv_core\.so\.' || true)
            if [ -n "$opencv_soname" ]; then
                sover=${opencv_soname##*.so.}
                [[ "$sover" =~ ^[0-9]+$ ]] || { echo "_nfpm: cannot read libopencv_core soversion from {{ config }}" >&2; exit 1; }
                printf -v OPENCV_DEPENDS '      - "opencv>=%s"\n      - "opencv<%s"' \
                    "$((sover / 100)).$((sover % 100))" "$((sover / 100)).$((sover % 100 + 1))"
            elif grep -q 'OPENCV_CONTENTS' {{ quote(config) }}; then
                OPENCV_DEPENDS="      - zlib"
                OPENCV_CONTENTS=""
                pkgname=$(sed -n 's/^name: "\(.*\)"$/\1/p' {{ quote(config) }})
                for license in LICENSE SoftFloat-COPYING.txt; do
                    [ -f "dist/opencv/$license" ] || { echo "_nfpm: dist/opencv/$license is not staged; run just build-rust with GAZE_OPENCV=static" >&2; exit 1; }
                    printf -v entry '  - src: "dist/opencv/%s"\n    dst: "/usr/share/licenses/%s/opencv/%s"\n    file_info:\n      mode: 0644\n' "$license" "$pkgname" "$license"
                    OPENCV_CONTENTS+=$entry
                done
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
    envsubst '$MULTIARCH $OPENCV_CONTENTS $OPENCV_DEPENDS $DEB_LIB_DEPENDS $RPM_LIB_DEPENDS $RPM_GSTREAMER_BASE $RPM_GSTREAMER_GOOD $RPM_GSTREAMER_PIPEWIRE $KCM_CONTENTS $KCM_DEPENDS $KCM_RECOMMENDS $LOCALE_CONTENTS' < {{ quote(config) }} > "$tmp_config"
    {{ quote(nfpm) }} pkg -f "$tmp_config" --packager {{ format }} --target dist/packages
    rm -f "$tmp_config"

[private]
_dist-packages:
    mkdir -p dist/packages

# Assert every packaged format pins the opencv soversion it links, if any, so a bump fails the
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

    want_link() {
        local p
        for p in "$@"; do
            awk -F'\t' -v p="$p" '$1 == p && $2 == "l" { found = 1 } END { exit !found }' <<< "$rows" ||
                fail "$base does not ship $p as a symlink"
        done
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

    want_opencv() {
        local binary tmp sonames=""
        tmp=$(mktemp)
        for binary in "$@"; do
            tar -xOf "$pkg" "${binary#/}" > "$tmp"
            sonames+=$(objdump -p "$tmp" | awk '/NEEDED.*libopencv_/ { print $2 }')
        done
        rm -f "$tmp"
        if [ -z "$sonames" ]; then
            want_files "/usr/share/licenses/${base%%-[0-9]*}/opencv/LICENSE" \
                "/usr/share/licenses/${base%%-[0-9]*}/opencv/SoftFloat-COPYING.txt"
            want_depends zlib
            ! grep -q '^opencv' <<< "$depends" || fail "$base links opencv statically but still depends on it"
            ok "$base links opencv statically"
        elif grep -Eq '^opencv>=[0-9]+\.[0-9]+$' <<< "$depends" && grep -Eq '^opencv<[0-9]+\.[0-9]+$' <<< "$depends"; then
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
    want_files /usr/bin/gazed /usr/bin/gaze \
        /etc/dbus-1/system.d/com.gundulabs.Gaze.conf \
        /usr/share/polkit-1/actions/com.gundulabs.gaze.policy \
        /usr/lib/systemd/system/gazed.service
    want_size /usr/bin/gazed 100000
    want_size /usr/bin/gaze 100000
    want_config /etc/dbus-1/system.d/com.gundulabs.Gaze.conf
    if [ "{{ format }}" = deb ]; then
        want_files /usr/share/gaze/config.toml
    else
        want_files /etc/gaze/config.toml
        want_config /etc/gaze/config.toml
    fi
    want_scripts "$post" "$preun"
    want_depends "${gst[@]}"
    case "{{ format }}" in
    deb)
        want_files "/lib/{{ multiarch }}/security/pam_gaze.so" \
            "/lib/{{ multiarch }}/security/pam_gaze_grosshack.so" \
            /usr/lib/security/pam_gaze.so /usr/lib/security/pam_gaze_grosshack.so \
            /usr/share/pam-configs/gaze /usr/share/pam-configs/gaze-simultaneous
        want_size "/lib/{{ multiarch }}/security/pam_gaze.so" 20000
        want_link "/lib/{{ multiarch }}/security/pam_gaze_grosshack.so"
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
            /usr/share/gaze/gaze-gdm-camera.pp /usr/share/gaze/gaze-greeter-keyring.pp
        want_size /usr/lib64/security/pam_gaze.so 20000
        want_link /usr/lib64/security/pam_gaze_grosshack.so
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
        want_link /usr/lib/security/pam_gaze_grosshack.so
        want_scripts post_upgrade
        want_depends pam tpm2-tss
        want_opencv /usr/bin/gazed /usr/bin/gaze
        ;;
    esac
    want_files /usr/lib/gaze/libonnxruntime.so \
        /usr/share/licenses/gaze/onnxruntime/LICENSE /usr/share/licenses/gaze/onnxruntime/ThirdPartyNotices.txt
    want_size /usr/lib/gaze/libonnxruntime.so 1000000
    ok "$base ships its daemon, CPU runtime, PAM modules, polkit action, unit and config"

    load gaze-gui
    want_files /usr/bin/gaze-gui /usr/share/applications/com.gundulabs.Gaze.desktop \
        /usr/share/icons/hicolor/scalable/apps/com.gundulabs.Gaze.svg \
        /usr/share/metainfo/com.gundulabs.Gaze.metainfo.xml
    want_size /usr/bin/gaze-gui 100000
    want_depends "${gst[@]}"
    if [ "{{ format }}" = archlinux ]; then
        want_depends gtk4 libadwaita
        want_opencv /usr/bin/gaze-gui
    fi
    ok "$base ships its binary, desktop entry, icon and metainfo"

    load gaze-gnome-extension
    want_files /usr/share/gnome-shell/extensions/gaze@gundulabs.com/metadata.json \
        /usr/share/gnome-shell/extensions/gaze@gundulabs.com/extension.js \
        /usr/share/gnome-shell/extensions/gaze@gundulabs.com/prefs.js \
        /usr/share/gnome-shell/extensions/gaze@gundulabs.com/locale/de/LC_MESSAGES/gaze@gundulabs.com.mo \
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
        /usr/share/cinnamon/extensions/gaze@gundulabs.com/settings-schema.json \
        /usr/share/locale/de/LC_MESSAGES/gaze@gundulabs.com.mo
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
    want_files /usr/bin/gaze-kde-pam
    legacy_entry=/usr/share/plasma/systemsettings/externalmodules/gaze-face-unlock.desktop
    if grep -Eq '^/usr/lib(64)?/([^/]+/)?qt6/plugins/plasma/kcms/systemsettings/kcm_gaze\.so$' <<< "$paths"; then
        want_files /usr/share/applications/kcm_gaze.desktop \
            /usr/share/applications/com.gundulabs.Gaze.Settings.desktop \
            /usr/share/locale/de/LC_MESSAGES/kcm_gaze.mo \
            /usr/share/icons/hicolor/scalable/apps/com.gundulabs.Gaze.Settings.svg
        case "{{ format }}" in
        deb) want_depends qml6-module-org-kde-kirigami qml6-module-org-kde-kcmutils ;;
        rpm) want_depends kf6-kirigami ;;
        archlinux) want_depends kcmutils kirigami ;;
        esac
        ! grep -Fxq "$legacy_entry" <<< "$paths" \
            || fail "$base ships both the Plasma 6 page and the legacy entry; System Settings would list Gaze twice"
    elif [ "{{ format }}" = rpm ]; then
        fail "$base does not ship the System Settings plugin"
    else
        want_files "$legacy_entry"
    fi
    want_size /usr/bin/gaze-kde-pam 1000
    want_scripts "$post" "$preun" "$postun"
    if [ "{{ format }}" = archlinux ]; then
        want_scripts post_upgrade
    fi
    if grep -Fxq /etc/pam.d/kde-fingerprint <<< "$paths"; then
        fail "$base ships /etc/pam.d/kde-fingerprint, which plasma-workspace owns; the transaction will conflict"
    fi
    ok "$base ships the PAM helper and System Settings entry, and claims no plasma-owned file"

    load gaze-omarchy
    want_files /usr/bin/gaze-omarchy /etc/pam.d/gaze-omarchy-face \
        /usr/share/gaze/omarchy/Service.qml /usr/share/gaze/omarchy/LockView.qml \
        /usr/share/gaze/omarchy/FingerprintModel.js \
        /usr/share/gaze/omarchy/manifest.json /usr/share/gaze/omarchy/upstream-lock.json \
        /usr/share/gaze/omarchy/THIRD_PARTY_NOTICES.md
    want_config /etc/pam.d/gaze-omarchy-face
    want_size /usr/bin/gaze-omarchy 1000
    want_scripts "$post" "$preun"
    if [ "{{ format }}" = archlinux ]; then
        want_scripts post_upgrade
    fi
    ok "$base ships the Omarchy lock plugin, user manager and dedicated face PAM lane"

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
package format: build-rust build-selinux (build-kcm format) && (package-prebuilt format)

# Package already-built artifacts for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
package-prebuilt format: _dist-packages prepare-ort build-locales
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

    configs=(packaging/nfpm/gaze.yaml packaging/nfpm/gui.yaml packaging/nfpm/gnome-extension.yaml packaging/nfpm/cinnamon-extension.yaml packaging/nfpm/hyprlock.yaml packaging/nfpm/kde.yaml packaging/nfpm/omarchy.yaml)
    if [ "{{ format }}" = rpm ] && is_suse; then
        configs=(packaging/nfpm/gaze-opensuse.yaml packaging/nfpm/gui.yaml packaging/nfpm/gnome-extension-opensuse.yaml packaging/nfpm/cinnamon-extension.yaml packaging/nfpm/hyprlock-opensuse.yaml packaging/nfpm/kde.yaml packaging/nfpm/omarchy.yaml)
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
        scripts/fetch-ort.sh {{ quote(ort_version) }} "$ort_arch" \
            "$sources/onnxruntime-linux-${ort_arch}-{{ ort_version }}.tgz"
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
    export SCRIPTLET_MAIN_POST="$(cat packaging/scriptlets/postinst-rpm.sh)"
    export SCRIPTLET_EXTENSION_POST="$(cat packaging/scriptlets/postinst-gnome-extension.sh)"
    export SCRIPTLET_EXTENSION_POSTUN="$(cat packaging/scriptlets/postrm-gnome-extension.sh)"
    export SCRIPTLET_OMARCHY_PREUN="$(cat packaging/scriptlets/prerm-omarchy.sh)"
    export SCRIPTLET_OMARCHY_POST="$(cat packaging/scriptlets/postinst-omarchy.sh)"
    export SCRIPTLET_KDE_POST="$(cat packaging/scriptlets/postinst-kde.sh)"
    export SCRIPTLET_KDE_PREUN="$(cat packaging/scriptlets/prerm-kde.sh)"
    export SCRIPTLET_KDE_POSTUN="$(cat packaging/scriptlets/postrm-kde.sh)"
    export SCRIPTLET_HYPRLOCK_POST="$(cat packaging/scriptlets/postinst-hyprlock.sh)"
    export SCRIPTLET_HYPRLOCK_POSTUN="$(cat packaging/scriptlets/postrm-hyprlock.sh)"

    mkdir -p "{{ srpm_topdir }}/SPECS"
    spec="{{ srpm_topdir }}/SPECS/gaze.spec"
    envsubst '$VERSION $PACKAGE_RELEASE $ORT_VERSION $CHANGELOG_DATE $SCRIPTLET_MAIN_POST $SCRIPTLET_EXTENSION_POST $SCRIPTLET_EXTENSION_POSTUN $SCRIPTLET_OMARCHY_PREUN $SCRIPTLET_OMARCHY_POST $SCRIPTLET_HYPRLOCK_POST $SCRIPTLET_HYPRLOCK_POSTUN $SCRIPTLET_KDE_POST $SCRIPTLET_KDE_PREUN $SCRIPTLET_KDE_POSTUN' \
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
test: prepare-ort prepare-opencv
    @{{ gui_notice }}
    {{ opencv_env }} cargo test --workspace {{ gui_exclude }} --release
    bash scripts/test-keyring-pam.sh
    bash scripts/test-kwallet-pam.sh

# Run the inference tests, plus the ignored OpenVINO session test, against Intel's OpenVINO runtime
[group("checks")]
test-openvino: prepare-ort-openvino prepare-opencv
    ORT_DYLIB_PATH={{ quote(ort_openvino_dir / "lib/libonnxruntime.so") }} \
    LD_LIBRARY_PATH={{ quote(ort_openvino_dir / "lib") }} \
    XDG_CACHE_HOME={{ quote(ort_openvino_dir / "cache") }} \
    {{ opencv_env }} cargo test -p gaze-vision --release -- --include-ignored inference::

# Check dependencies for known security advisories
[group("checks")]
audit:
    cargo audit

# Verify PAM links only system libraries and the main module's TPM libraries
[group("checks")]
check-pam-link:
    scripts/check-pam-link.sh target/release/libpam_gaze.so

# Run clippy lints across the workspace
[group("checks")]
lint: prepare-opencv
    @{{ gui_notice }}
    {{ opencv_env }} cargo clippy --workspace {{ gui_exclude }} --all-targets -- -D warnings

# Lint every tracked shell script, including the installer and package scriptlets
[group("checks")]
lint-shell:
    git ls-files -z '*.sh' integrations/kde/gaze-kde-pam integrations/omarchy/gaze-omarchy scripts/pre-commit | xargs -0 shellcheck -S warning

# Check formatting (does not write)
[group("checks")]
fmt-check:
    cargo fmt --all -- --check

# Check the translation catalogs and the metadata generated from them
[group("checks")]
check-translations:
    python3 scripts/sync-translations.py --check

# Regenerate the translated desktop, metainfo, polkit and KCM metadata
[group("dev")]
sync-translations:
    python3 scripts/sync-translations.py

# Apply formatting
[group("dev")]
fmt:
    cargo fmt --all

# Also enables TPM template encryption when a TPM is present; GAZE_DEV_TPM=0 skips it.
# Link the installed system runtime to this checkout's release build
[group("dev")]
dev-link-system: build-rust build-locales
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
    bun install --frozen-lockfile
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

# ── native Arch container (including derivative repository snapshots) ─────────

arch_engine := env("GAZE_CONTAINER_ENGINE", "docker")
arch_mirror := env("GAZE_ARCH_MIRROR", "https://geo.mirror.pkgbuild.com")
# One image per mirror, so switching mirrors keeps each one's package sync and target cache.
arch_image := env("GAZE_ARCH_IMAGE", "gaze-build-arch:" + replace_regex(sha256(arch_mirror), '^(.{12}).*$', '$1'))
arch_repo := replace_regex(arch_image, ':[^:/]*$', '')
arch_sync_fmt := '{{ index .Config.Labels "com.gundulabs.gaze.arch-sync" }}'
arch_mirror_fmt := '{{ index .Config.Labels "com.gundulabs.gaze.arch-mirror" }}'
# `just arch` warns once the image's package sync is this many days old.
arch_stale_days := "7"

# Build the x86_64 Arch environment against the selected mirror; `just arch-refresh` re-syncs it
[group("docker")]
arch-image: (_arch-build "keep")

# Re-sync the Arch image with its mirror and remove the image and target cache it replaces
[group("docker")]
arch-refresh:
    #!/usr/bin/env bash
    set -euo pipefail
    engine={{ quote(arch_engine) }}
    image={{ quote(arch_image) }}
    old=$("$engine" image inspect --format {{ quote("{{.Id}}") }} "$image" 2>/dev/null || true)
    {{ quote(just_executable()) }} --justfile {{ quote(justfile()) }} _arch-build new
    new=$("$engine" image inspect --format {{ quote("{{.Id}}") }} "$image")
    if [ -n "$old" ] && [ "$old" != "$new" ]; then
        old_key=${old#sha256:}
        "$engine" volume rm "gaze-arch-target-${old_key:0:16}" >/dev/null 2>&1 || true
        "$engine" image rm "$old" >/dev/null 2>&1 || true
    fi

# Remove Arch target-cache volumes that no tagged Arch image uses
[group("docker")]
arch-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    engine={{ quote(arch_engine) }}
    keep=" "
    for id in $("$engine" images --no-trunc --format {{ quote("{{.ID}}") }} {{ quote(arch_repo) }}); do
        id=${id#sha256:}
        keep+="gaze-arch-target-${id:0:16} "
    done
    for volume in $("$engine" volume ls -q); do
        case $volume in gaze-arch-target-*) ;; *) continue ;; esac
        case $keep in *" $volume "*) continue ;; esac
        if "$engine" volume rm "$volume" >/dev/null; then
            echo "removed $volume"
        else
            echo "kept $volume (in use)"
        fi
    done

# `keep` reuses the image's sync stamp so the cached package layer stays valid; `new` re-syncs.
[private]
_arch-build sync:
    #!/usr/bin/env bash
    set -euo pipefail
    engine={{ quote(arch_engine) }}
    image={{ quote(arch_image) }}
    mirror={{ quote(arch_mirror) }}
    stamp=$(date -u +%s)
    if [ {{ quote(sync) }} = keep ]; then
        # An image synced against another mirror (a shared GAZE_ARCH_IMAGE) gets a new stamp.
        old_mirror=$("$engine" image inspect --format {{ quote(arch_mirror_fmt) }} "$image" 2>/dev/null || true)
        old_stamp=$("$engine" image inspect --format {{ quote(arch_sync_fmt) }} "$image" 2>/dev/null || true)
        if [ "$old_mirror" = "$mirror" ] && [[ $old_stamp =~ ^[0-9]+$ ]]; then
            stamp=$old_stamp
        fi
    fi
    "$engine" build --build-arg "ARCH_MIRROR=$mirror" --build-arg "ARCH_SYNC=$stamp" \
        -t "$image" -f packaging/docker/Dockerfile.arch packaging/docker

# Run a native Arch target, e.g. `just arch package archlinux`
[group("docker")]
[positional-arguments]
arch target *args: arch-image
    #!/usr/bin/env bash
    set -euo pipefail
    engine={{ quote(arch_engine) }}
    image={{ quote(arch_image) }}
    if [ -n "${ARCH:-}" ] && [ "$ARCH" != x86_64 ]; then
        echo "arch: the Arch image builds x86_64 only, got ARCH=$ARCH" >&2
        exit 1
    fi
    image_id=$("$engine" image inspect --format {{ quote("{{.Id}}") }} "$image")
    key=${image_id#sha256:}
    stamp=$("$engine" image inspect --format {{ quote(arch_sync_fmt) }} "$image")
    if [[ $stamp =~ ^[0-9]+$ ]]; then
        age=$(( ($(date -u +%s) - stamp) / 86400 ))
        if [ "$age" -ge {{ arch_stale_days }} ]; then
            echo "arch: packages were synced with {{ arch_mirror }} $age days ago; run 'just arch-refresh' to update them" >&2
        fi
    fi
    # label=disable lets the container read the checkout on SELinux hosts without
    # relabelling it, as --privileged does for the Ubuntu recipe. entrypoint.sh skips
    # the HOST_UID chown when container root is not host root (rootless engines).
    "$engine" run --rm --security-opt label=disable \
        -v {{ quote(justfile_directory() + ":/work") }} \
        -v gaze-arch-cargo-registry:/root/.cargo/registry \
        -v gaze-arch-cargo-git:/root/.cargo/git \
        -v "gaze-arch-target-${key:0:16}:/work/target" \
        -e CARGO_BUILD_JOBS -e RUSTFLAGS -e VERSION -e PACKAGE_RELEASE \
        -e GAZE_GUI -e GAZE_KCM -e ORT_VERSION \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        "$image_id" "$@"
