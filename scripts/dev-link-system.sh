#!/bin/sh
set -eu

usage() {
    cat <<'EOF'
Usage: scripts/dev-link-system.sh enable|disable|status

Point the locally installed Gaze runtime at this checkout's release artifacts.

Run as root after building release artifacts as your normal user:

    cargo build --workspace --release
    sudo scripts/dev-link-system.sh enable

This links:
  - /usr/bin/gazed, /usr/bin/gaze, /usr/bin/gaze-gui
  - installed PAM modules
  - system and current-user GNOME extension files
  - the installed GNOME settings schema

Privileged runtime files are copied to system-labeled paths first, then the
installed entry points are linked to those copies. This avoids SELinux blocking
systemd or PAM from executing files directly under your home directory.

It also installs a systemd drop-in that clears the packaged unit's
InaccessiblePaths=/home /root rule for local development.

When a TPM is present, `enable` turns on TPM template encryption
([storage] encrypt_templates = true); gazed seals a key to this machine's TPM and
encrypts enrolled templates on restart. Set GAZE_DEV_TPM=0 to skip this. `disable`
turns it back off (templates enrolled while on stay encrypted; re-link or wipe
/var/lib/gaze/users to recover).

Use `disable` to restore files backed up during `enable`.
EOF
}

die() {
    printf '%s\n' "$*" >&2
    exit 1
}

need_root() {
    [ "$(id -u)" -eq 0 ] || die "Run this script with sudo."
}

repo_root() {
    CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P
}

REPO=$(repo_root)
TARGET="$REPO/target/release"
CONFIG_FILE=/etc/gaze/config.toml
USERS_DIR=/var/lib/gaze/users
TPM_STATE_DIR=/var/lib/gaze/tpm
BACKUP_DIR=/usr/local/share/gaze-dev/originals
LOCAL_BIN_DIR=/usr/local/bin
SYSTEMD_DROPIN=/etc/systemd/system/gazed.service.d/zz-gaze-dev-checkout.conf
LEGACY_SYSTEMD_DROPIN=/etc/systemd/system/gazed.service.d/dev-checkout.conf
SYSTEM_EXTENSION_DIR=/usr/share/gnome-shell/extensions/gaze@gundulabs.com
SCHEMA_SRC="$REPO/packaging/config/org.gnome.shell.extensions.gaze.gschema.xml"
SCHEMA_DST=/usr/share/glib-2.0/schemas/org.gnome.shell.extensions.gaze.gschema.xml

artifact() {
    printf '%s/%s' "$TARGET" "$1"
}

require_artifacts() {
    missing=0
    for file in \
        "$(artifact gazed)" \
        "$(artifact gaze)" \
        "$(artifact gaze-gui)" \
        "$(artifact libpam_gaze.so)" \
        "$(artifact libpam_gaze_grosshack.so)"
    do
        if [ ! -e "$file" ]; then
            printf 'Missing build artifact: %s\n' "$file" >&2
            missing=1
        fi
    done

    [ "$missing" -eq 0 ] || die "Build first: cargo build --workspace --release"
}

backup_name() {
    printf '%s' "$1" | tr '/ ' '__'
}

backup_and_link() {
    src=$1
    dst=$2
    name=$(backup_name "$dst")
    backup="$BACKUP_DIR/$name"

    [ -e "$src" ] || die "Missing source: $src"
    install -d "$(dirname -- "$dst")" "$BACKUP_DIR"

    current=
    should_backup=1
    if [ -L "$dst" ]; then
        current=$(readlink "$dst" || true)
        case "$current" in
            "$REPO"/*|"$LOCAL_BIN_DIR"/*) should_backup=0 ;;
        esac
    fi

    if [ "$current" != "$src" ] && [ "$should_backup" -eq 1 ] && [ ! -e "$backup" ] && { [ -e "$dst" ] || [ -L "$dst" ]; }; then
        cp -a "$dst" "$backup"
    fi

    rm -f "$dst"
    ln -s "$src" "$dst"
    printf 'linked %s -> %s\n' "$dst" "$src"
}

backup_and_install() {
    src=$1
    dst=$2
    mode=$3
    name=$(backup_name "$dst")
    backup="$BACKUP_DIR/$name"

    [ -e "$src" ] || die "Missing source: $src"
    install -d "$(dirname -- "$dst")" "$BACKUP_DIR"

    should_backup=1
    if [ -L "$dst" ]; then
        current=$(readlink "$dst" || true)
        case "$current" in
            "$REPO"/*|"$LOCAL_BIN_DIR"/*) should_backup=0 ;;
        esac
    fi

    if [ "$should_backup" -eq 1 ] && [ ! -e "$backup" ] && { [ -e "$dst" ] || [ -L "$dst" ]; }; then
        cp -a "$dst" "$backup"
    fi

    rm -f "$dst"
    install -m "$mode" "$src" "$dst"
    if command -v restorecon >/dev/null 2>&1; then
        restorecon "$dst" >/dev/null 2>&1 || true
    fi
    printf 'installed %s from %s\n' "$dst" "$src"
}

restore_or_remove() {
    dst=$1
    name=$(backup_name "$dst")
    backup="$BACKUP_DIR/$name"

    if [ -e "$backup" ] || [ -L "$backup" ]; then
        rm -f "$dst"
        cp -a "$backup" "$dst"
        printf 'restored %s\n' "$dst"
    elif [ -L "$dst" ]; then
        rm -f "$dst"
        printf 'removed %s\n' "$dst"
    fi
}

link_binaries() {
    backup_and_install "$(artifact gazed)" "$LOCAL_BIN_DIR/gazed" 0755
    backup_and_install "$(artifact gaze)" "$LOCAL_BIN_DIR/gaze" 0755
    backup_and_install "$(artifact gaze-gui)" "$LOCAL_BIN_DIR/gaze-gui" 0755
    backup_and_link "$LOCAL_BIN_DIR/gazed" /usr/bin/gazed
    backup_and_link "$LOCAL_BIN_DIR/gaze" /usr/bin/gaze
    backup_and_link "$LOCAL_BIN_DIR/gaze-gui" /usr/bin/gaze-gui
}

restore_binaries() {
    restore_or_remove /usr/bin/gazed
    restore_or_remove /usr/bin/gaze
    restore_or_remove /usr/bin/gaze-gui
    restore_or_remove "$LOCAL_BIN_DIR/gazed"
    restore_or_remove "$LOCAL_BIN_DIR/gaze"
    restore_or_remove "$LOCAL_BIN_DIR/gaze-gui"
}

link_pam_dir() {
    dir=$1
    [ -d "$dir" ] || return 1
    backup_and_install "$(artifact libpam_gaze.so)" "$dir/pam_gaze.so" 0755
    backup_and_install "$(artifact libpam_gaze_grosshack.so)" "$dir/pam_gaze_grosshack.so" 0755
    return 0
}

link_pam_modules() {
    linked=0
    multiarch=
    if command -v gcc >/dev/null 2>&1; then
        multiarch=$(gcc -print-multiarch 2>/dev/null || true)
    fi

    for dir in \
        "/lib/$multiarch/security" \
        "/usr/lib/$multiarch/security" \
        /usr/lib64/security \
        /usr/lib/security
    do
        case "$dir" in
            /lib//security|/usr/lib//security) continue ;;
        esac

        if [ -e "$dir/pam_gaze.so" ] || [ -e "$dir/pam_gaze_grosshack.so" ]; then
            link_pam_dir "$dir" && linked=1
        fi
    done

    if [ "$linked" -eq 0 ]; then
        for dir in "/lib/$multiarch/security" /usr/lib64/security /usr/lib/security; do
            case "$dir" in
                /lib//security) continue ;;
            esac
            if link_pam_dir "$dir"; then
                linked=1
                break
            fi
        done
    fi

    [ "$linked" -eq 1 ] || die "Could not find a PAM security module directory."
}

restore_pam_modules() {
    multiarch=
    if command -v gcc >/dev/null 2>&1; then
        multiarch=$(gcc -print-multiarch 2>/dev/null || true)
    fi

    for dir in \
        "/lib/$multiarch/security" \
        "/usr/lib/$multiarch/security" \
        /usr/lib64/security \
        /usr/lib/security
    do
        case "$dir" in
            /lib//security|/usr/lib//security) continue ;;
        esac
        restore_or_remove "$dir/pam_gaze.so"
        restore_or_remove "$dir/pam_gaze_grosshack.so"
    done
}

link_extension_files() {
    dir=$1
    install -d "$dir"
    backup_and_install "$REPO/gnome-shell-extension/metadata.json" "$dir/metadata.json" 0644
    backup_and_install "$REPO/gnome-shell-extension/extension.js" "$dir/extension.js" 0644
    backup_and_install "$REPO/gnome-shell-extension/prefs.js" "$dir/prefs.js" 0644
}

restore_extension_files() {
    dir=$1
    restore_or_remove "$dir/metadata.json"
    restore_or_remove "$dir/extension.js"
    restore_or_remove "$dir/prefs.js"
}

sudo_user_home() {
    [ -n "${SUDO_USER:-}" ] || return 1
    [ "$SUDO_USER" != root ] || return 1
    getent passwd "$SUDO_USER" | cut -d: -f6
}

link_gnome_extension() {
    link_extension_files "$SYSTEM_EXTENSION_DIR"
    backup_and_install "$SCHEMA_SRC" "$SCHEMA_DST" 0644

    if home=$(sudo_user_home); then
        user_extension_dir="$home/.local/share/gnome-shell/extensions/gaze@gundulabs.com"
        sudo_user_group=$(id -gn "$SUDO_USER")
        install -d -o "$SUDO_USER" -g "$sudo_user_group" "$user_extension_dir"
        link_extension_files "$user_extension_dir"
        chown "$SUDO_USER:$sudo_user_group" \
            "$user_extension_dir/metadata.json" \
            "$user_extension_dir/extension.js" \
            "$user_extension_dir/prefs.js"
    fi

    if command -v glib-compile-schemas >/dev/null 2>&1; then
        glib-compile-schemas /usr/share/glib-2.0/schemas
    fi
}

restore_gnome_extension() {
    restore_extension_files "$SYSTEM_EXTENSION_DIR"
    restore_or_remove "$SCHEMA_DST"

    if home=$(sudo_user_home); then
        restore_extension_files "$home/.local/share/gnome-shell/extensions/gaze@gundulabs.com"
    fi

    if command -v glib-compile-schemas >/dev/null 2>&1; then
        glib-compile-schemas /usr/share/glib-2.0/schemas
    fi
}

install_systemd_dropin() {
    install -d "$(dirname -- "$SYSTEMD_DROPIN")"
    cat >"$SYSTEMD_DROPIN" <<'EOF'
[Service]
# Keep this drop-in lexically late so it wins over older local ExecStart overrides.
ExecStart=
ExecStart=/usr/bin/gazed

# The packaged unit hides /home, but dev symlink targets live in the checkout.
InaccessiblePaths=
EOF
    rm -f "$LEGACY_SYSTEMD_DROPIN"
    systemctl daemon-reload
    systemctl restart gazed
}

remove_systemd_dropin() {
    rm -f "$SYSTEMD_DROPIN"
    rm -f "$LEGACY_SYSTEMD_DROPIN"
    systemctl daemon-reload
    systemctl restart gazed || true
}

show_status() {
    printf 'repo: %s\n' "$REPO"
    for path in \
        /usr/bin/gazed \
        /usr/bin/gaze \
        /usr/bin/gaze-gui \
        "$LOCAL_BIN_DIR/gazed" \
        "$LOCAL_BIN_DIR/gaze" \
        "$LOCAL_BIN_DIR/gaze-gui" \
        "$SYSTEM_EXTENSION_DIR/extension.js" \
        "$SYSTEM_EXTENSION_DIR/prefs.js" \
        "$SCHEMA_DST"
    do
        if [ -L "$path" ]; then
            printf '%s -> %s\n' "$path" "$(readlink "$path")"
        elif [ -e "$path" ]; then
            printf '%s is not a symlink\n' "$path"
        else
            printf '%s is missing\n' "$path"
        fi
    done
    systemctl show gazed -p DropInPaths -p ExecStart -p InaccessiblePaths 2>/dev/null || true

    if tpm_present; then tpm=$([ -e /dev/tpmrm0 ] && echo /dev/tpmrm0 || echo /dev/tpm0); else tpm=none; fi
    printf 'tpm device: %s\n' "$tpm"
    flag=$(awk '/^[[:space:]]*encrypt_templates[[:space:]]*=/ {print $3; f=1} END{if(!f) print "unset"}' "$CONFIG_FILE" 2>/dev/null || echo unknown)
    printf 'encrypt_templates: %s\n' "$flag"
    printf 'sealed key: %s\n' "$([ -e "$TPM_STATE_DIR/dek.priv" ] && echo present || echo absent)"
    printf 'encrypted templates on disk: %s\n' "$(encrypted_template_count)"
}

tpm_present() {
    [ -e /dev/tpmrm0 ] || [ -e /dev/tpm0 ]
}

# Set [storage] encrypt_templates to true|false in config.toml, preserving the
# rest of the file and creating the table if it is missing.
set_storage_encrypt() {
    val=$1
    install -d -m 0755 "$(dirname -- "$CONFIG_FILE")"
    [ -e "$CONFIG_FILE" ] || : >"$CONFIG_FILE"
    tmp=$(mktemp)
    awk -v val="$val" '
        /^[[:space:]]*\[storage\][[:space:]]*$/ { print; in_s=1; next }
        /^[[:space:]]*\[/ { if (in_s && !done) { print "encrypt_templates = " val; done=1 } in_s=0; print; next }
        in_s && /^[[:space:]]*encrypt_templates[[:space:]]*=/ { if (!done) { print "encrypt_templates = " val; done=1 } next }
        { print }
        END {
            if (in_s && !done) print "encrypt_templates = " val
            else if (!done) { print ""; print "[storage]"; print "encrypt_templates = " val }
        }
    ' "$CONFIG_FILE" >"$tmp"
    install -m 0600 "$tmp" "$CONFIG_FILE"
    rm -f "$tmp"
    if command -v restorecon >/dev/null 2>&1; then
        restorecon "$CONFIG_FILE" >/dev/null 2>&1 || true
    fi
}

encrypted_template_count() {
    [ -d "$USERS_DIR" ] || { echo 0; return 0; }
    find "$USERS_DIR" -type f -name '*.bin' 2>/dev/null | while IFS= read -r f; do
        if [ "$(head -c4 "$f" 2>/dev/null)" = "GZE1" ]; then printf '.\n'; fi
    done | wc -l | tr -d ' '
}

setup_tpm_encryption() {
    if [ "${GAZE_DEV_TPM:-1}" = "0" ]; then
        printf 'GAZE_DEV_TPM=0 set; leaving template encryption disabled.\n'
        set_storage_encrypt false
        return 0
    fi
    if ! tpm_present; then
        printf 'WARNING: no TPM device (/dev/tpmrm0); leaving template encryption disabled.\n' >&2
        printf '         gazed fails closed if encrypt_templates=true without a usable TPM.\n' >&2
        set_storage_encrypt false
        return 0
    fi
    set_storage_encrypt true
    printf 'TPM template encryption ON in %s; gazed seals a key to this TPM and encrypts templates on restart.\n' "$CONFIG_FILE"
}

teardown_tpm_encryption() {
    [ -e "$CONFIG_FILE" ] || return 0
    set_storage_encrypt false
    n=$(encrypted_template_count)
    if [ "$n" -gt 0 ]; then
        printf '\nWARNING: %s encrypted template file(s) remain under %s.\n' "$n" "$USERS_DIR" >&2
        printf '         The package gazed cannot read them. Re-link the dev build to keep using\n' >&2
        printf '         them, or wipe and re-enroll: sudo rm -rf %s\n' "$USERS_DIR" >&2
    fi
}

cmd=${1:-}
case "$cmd" in
    enable)
        need_root
        require_artifacts
        link_binaries
        link_pam_modules
        link_gnome_extension
        setup_tpm_encryption
        install_systemd_dropin
        printf '\nGaze is linked to this checkout. Rebuild after switching branches, then restart gazed.\n'
        printf 'Restart GNOME Shell or log out/in for extension.js changes. Reopen preferences for prefs.js changes.\n'
        ;;
    disable)
        need_root
        restore_binaries
        restore_pam_modules
        restore_gnome_extension
        teardown_tpm_encryption
        remove_systemd_dropin
        ;;
    status)
        show_status
        ;;
    -h|--help|help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
