use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(path: &str) -> String {
    fs::read_to_string(repo().join(path)).unwrap()
}

#[test]
fn client_crates_disable_detection_to_keep_onnxruntime_out() {
    for manifest in [
        "gaze-cli/Cargo.toml",
        "gaze-gui/Cargo.toml",
        "pam-gaze-core/Cargo.toml",
        "pam-gaze/Cargo.toml",
        "pam-gaze-grosshack/Cargo.toml",
    ] {
        let cargo = read(manifest);
        assert!(
            cargo.lines().any(|line| {
                line.contains("gaze-core") && line.contains("default-features = false")
            }),
            "{manifest} must disable gaze-core default features"
        );
    }
}

#[test]
fn release_build_keeps_daemon_and_clients_in_separate_cargo_invocations() {
    let justfile = read("Justfile");
    let recipe = justfile
        .split("build-rust:")
        .nth(1)
        .unwrap()
        .split("# Compile the SELinux")
        .next()
        .unwrap();

    assert!(recipe.contains("cargo build -p gaze --release"));
    assert!(recipe.contains(
        "cargo build -p gaze-cli -p gaze-gui -p pam-gaze -p pam-gaze-grosshack --release"
    ));
    assert!(!recipe.contains("--workspace"));
}

#[test]
fn package_installs_pam_modules_in_legacy_and_multiarch_locations() {
    let manifest = read("packaging/nfpm.yaml");
    for path in [
        "/lib/${MULTIARCH}/security/pam_gaze.so",
        "/usr/lib/security/pam_gaze.so",
        "/usr/lib64/security/pam_gaze.so",
        "/lib/${MULTIARCH}/security/pam_gaze_grosshack.so",
        "/usr/lib/security/pam_gaze_grosshack.so",
        "/usr/lib64/security/pam_gaze_grosshack.so",
    ] {
        assert!(manifest.contains(path), "package no longer installs {path}");
    }
}

#[test]
fn flatpak_preview_retains_pipewire_socket_access() {
    assert!(
        read("packaging/flatpak/com.gundulabs.Gaze.yml")
            .contains("--filesystem=xdg-run/pipewire-0")
    );
}

#[test]
fn dev_link_installs_and_restores_the_polkit_policy() {
    let script = read("scripts/dev-link-system.sh");
    assert!(
        script.contains("POLKIT_POLICY_DST=/usr/share/polkit-1/actions/com.gundulabs.gaze.policy")
    );
    assert!(script.contains("link_polkit_policy"));
    assert!(script.contains("restore_polkit_policy"));
}

#[test]
fn arch_postinstall_pam_edit_is_guarded_against_duplicates() {
    let script = read("packaging/postinst-arch.sh");
    assert!(script.contains("grep -q \"pam_gaze\" \"$pam_file\""));
    assert!(script.contains("auth        sufficient    pam_gaze.so"));
    assert!(script.contains("/etc/gaze/pam-arch.configured"));
}

#[test]
fn gdm_checks_enrollment_before_starting_face_pam_service() {
    let extension = read("gnome-shell-extension/extension.js");
    assert!(extension.contains("<method name=\"HasEnrolledFaces\">"));
    assert!(extension.contains("dbusProxy.HasEnrolledFacesRemote(userName"));
    assert!(extension.contains("result[0]"));
    assert!(extension.contains("self._startService(FACE_SERVICE_NAME)"));
}

#[test]
fn shell_scripts_parse_with_posix_sh() {
    for script in [
        "packaging/postinst-arch.sh",
        "packaging/postinst-deb.sh",
        "packaging/postinst-rpm.sh",
        "packaging/prerm-deb.sh",
        "scripts/dev-link-system.sh",
    ] {
        let status = Command::new("sh")
            .arg("-n")
            .arg(repo().join(script))
            .status()
            .unwrap();
        assert!(status.success(), "{script} has invalid shell syntax");
    }
}
