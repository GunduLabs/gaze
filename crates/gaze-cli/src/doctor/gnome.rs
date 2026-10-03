// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    match std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty()) {
        Some(value) => dirs.extend(std::env::split_paths(&value)),
        None => dirs.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]),
    }
    dirs
}

// Nix installs the extension's schema inside the extension directory rather
// than into the system schema path, so `gsettings` cannot see it unaided.
pub(super) fn extension_schema_dir() -> Option<PathBuf> {
    extension_schema_dir_in(&xdg_data_dirs())
}

pub(super) fn extension_dir(data_dir: &Path) -> PathBuf {
    data_dir
        .join("gnome-shell")
        .join("extensions")
        .join(GNOME_EXTENSION_ID)
}

pub(super) fn extension_schema_dir_in(data_dirs: &[PathBuf]) -> Option<PathBuf> {
    data_dirs
        .iter()
        .map(|dir| extension_dir(dir).join("schemas"))
        .find(|dir| dir.join("gschemas.compiled").exists())
}

/// Whether the extension files are on disk, which separates "the package is
/// missing" from "GNOME Shell has not picked the package up yet".
pub(super) fn extension_installed() -> bool {
    extension_installed_in(&xdg_data_dirs())
}

pub(super) fn extension_installed_in(data_dirs: &[PathBuf]) -> bool {
    data_dirs
        .iter()
        .any(|dir| extension_dir(dir).join("metadata.json").exists())
}

pub(super) fn extension_setting(key: &str) -> std::io::Result<(bool, String)> {
    let schema_dir = extension_schema_dir();
    let env: Vec<(&str, &OsStr)> = schema_dir
        .as_deref()
        .map(|dir| vec![("GSETTINGS_SCHEMA_DIR", dir.as_os_str())])
        .unwrap_or_default();
    command_output_env("gsettings", &["get", GNOME_EXTENSION_SCHEMA, key], &env)
}

/// `None` only when `dconf` itself could not answer. An unset key reads back as an empty
/// string, which callers layering one db over another must tell apart from a real value.
pub(super) fn dconf_read_with_profile(
    tag: &str,
    profile_body: &str,
    config_home: Option<&Path>,
    key: &str,
) -> Option<String> {
    let profile =
        std::env::temp_dir().join(format!("gaze-doctor-{tag}-{}.profile", std::process::id()));
    fs::write(&profile, profile_body).ok()?;
    let mut env: Vec<(&str, &OsStr)> = vec![("DCONF_PROFILE", profile.as_os_str())];
    if let Some(home) = config_home {
        env.push(("XDG_CONFIG_HOME", home.as_os_str()));
    }
    let result = command_output_env("dconf", &["read", key], &env);
    let _ = fs::remove_file(&profile);
    match result {
        Ok((true, value)) => Some(value.trim().to_string()),
        _ => None,
    }
}

pub(super) fn gdm_system_dconf_read(key: &str) -> Option<String> {
    dconf_read_with_profile(
        GDM_DCONF_PROFILE,
        &format!("system-db:{GDM_DCONF_PROFILE}\n"),
        None,
        key,
    )
}

pub(super) fn gdm_user_db_read(dir: &Path, key: &str) -> Option<String> {
    dconf_read_with_profile("gdm-user", "user-db:user\n", Some(dir), key)
        .filter(|value| !value.is_empty())
}

/// The greeter runs as `gdm` with its own `XDG_CONFIG_HOME`, and the path differs by
/// distribution and by seat, so every candidate holding a db has to be considered.
pub(super) fn gdm_greeter_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for home in GDM_HOME_DIRS {
        let home = Path::new(home);
        if !home.is_dir() {
            continue;
        }
        dirs.push(home.join(".config"));
        if let Ok(entries) = fs::read_dir(home) {
            for entry in entries.flatten() {
                dirs.push(entry.path().join("config"));
            }
        }
    }
    dirs.retain(|dir| dir.join("dconf/user").is_file());
    dirs.sort();
    dirs.dedup();
    dirs
}

/// `user-db:user` leads the greeter profile, so whatever GDM has written for itself outranks
/// every `system-db` keyfile. Reading only `system-db:gdm` reports what Gaze installed rather
/// than what the greeter resolves, which is how a disabled extension system passed as ready.
pub(super) fn gdm_greeter_dconf_read(key: &str) -> Option<String> {
    for dir in gdm_greeter_config_dirs() {
        if let Some(value) = gdm_user_db_read(&dir, key) {
            return Some(value);
        }
    }
    gdm_system_dconf_read(key)
}

/// Which greeter db holds `key`, for a fix that has to name the file it must be cleared from.
pub(super) fn gdm_greeter_dconf_source(key: &str) -> Option<PathBuf> {
    gdm_greeter_config_dirs()
        .into_iter()
        .find(|dir| gdm_user_db_read(dir, key).is_some())
        .map(|dir| dir.join("dconf/user"))
}

pub(super) fn dconf_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// The value GDM itself sees, which a NixOS configuration sets without `GDM_FACE_OVERRIDE_PATH`.
pub(super) fn gdm_face_auth_from_dconf() -> Option<bool> {
    if !Path::new(GDM_DCONF_PROFILE_PATH).exists() {
        return None;
    }
    dconf_bool(&gdm_greeter_dconf_read(GDM_DCONF_FACE_AUTH_KEY)?)
}

pub(super) fn profile_reads_system_db(contents: &str, db: &str) -> bool {
    let wanted = format!("system-db:{db}");
    contents.lines().any(|line| line.trim() == wanted)
}

pub(super) fn extensions_include(value: &str, uuid: &str) -> bool {
    value
        .trim_matches(|c: char| c == '[' || c == ']')
        .split(',')
        .any(|entry| entry.trim().trim_matches(|c| c == '\'' || c == '"') == uuid)
}

pub(super) enum GdmGreeterReadiness {
    Ready,
    ProfileMissingSystemDb,
    CompiledDbMissing,
    ExtensionNotEnabled,
    /// `Some` names the greeter db holding the key, `None` means a `system-db` layer set it.
    ExtensionsDisabled(Option<PathBuf>),
    Unverifiable(String),
}

pub(super) fn gdm_greeter_readiness() -> GdmGreeterReadiness {
    match fs::read_to_string(GDM_DCONF_PROFILE_PATH) {
        Ok(contents) if !profile_reads_system_db(&contents, GDM_DCONF_PROFILE) => {
            return GdmGreeterReadiness::ProfileMissingSystemDb;
        }
        Ok(_) => {}
        Err(err) => {
            return GdmGreeterReadiness::Unverifiable(format!(
                "could not read {GDM_DCONF_PROFILE_PATH}: {err}"
            ));
        }
    }

    if !Path::new(GDM_COMPILED_DB_PATH).exists() {
        return GdmGreeterReadiness::CompiledDbMissing;
    }

    match gdm_greeter_dconf_read(GDM_ENABLED_EXTENSIONS_KEY) {
        Some(value) if extensions_include(&value, GNOME_EXTENSION_ID) => {}
        Some(_) => return GdmGreeterReadiness::ExtensionNotEnabled,
        None => {
            return GdmGreeterReadiness::Unverifiable(
                "`dconf read` against the GDM database failed".to_string(),
            );
        }
    }

    // Checked after the list because it overrides it: gnome-shell stops its whole extension
    // system, so the greeter loads nothing however the extension is enabled.
    if gdm_greeter_dconf_read(GDM_DISABLE_EXTENSIONS_KEY)
        .as_deref()
        .and_then(dconf_bool)
        == Some(true)
    {
        return GdmGreeterReadiness::ExtensionsDisabled(gdm_greeter_dconf_source(
            GDM_DISABLE_EXTENSIONS_KEY,
        ));
    }

    GdmGreeterReadiness::Ready
}

pub(super) fn gdm_selinux_fix() -> String {
    let path = selinux::policy_path(selinux::GDM_CAMERA_MODULE);
    if Path::new(&path).exists() {
        format!("Run `sudo semodule -i {path}`, then reboot.")
    } else {
        format!("Reinstall the Gaze GNOME extension package to restore {path}, then reboot.")
    }
}

pub(super) fn check_gdm_selinux(report: &mut Report) {
    if !selinux::is_enforcing() {
        return;
    }

    report_gdm_camera_policy(report, selinux::module_state(selinux::GDM_CAMERA_MODULE));
}

pub(super) fn report_gdm_camera_policy(report: &mut Report, policy: ModuleState) {
    let module = selinux::GDM_CAMERA_MODULE;
    match policy {
        ModuleState::Loaded => report.pass(
            "GDM camera SELinux policy",
            format!("{module} is loaded, so the greeter can open the camera"),
        ),
        ModuleState::NotLoaded => report.error(
            "GDM camera SELinux policy",
            format!(
                "SELinux is enforcing and {module} is not loaded, so the GDM greeter is denied the camera and the login screen never scans"
            ),
            gdm_selinux_fix(),
        ),
        ModuleState::NeedsRoot => report.warning(
            "GDM camera SELinux policy",
            format!(
                "SELinux is enforcing, and whether {module} is loaded could not be \
                 checked without root"
            ),
            "Run `sudo gaze doctor` to read the loaded module list.",
        ),
        ModuleState::Unverifiable(why) => report.warning(
            "GDM camera SELinux policy",
            format!("SELinux is enforcing, but the loaded module list could not be read: {why}"),
            format!(
                "Run `semodule -l | grep {module}`; if it prints nothing, {}",
                gdm_selinux_fix()
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gdm_profile_without_the_system_db_is_detected() {
        let debian = "user-db:user\nsystem-db:gdm\nfile-db:/usr/share/gdm/greeter-dconf-defaults\n";
        assert!(profile_reads_system_db(debian, "gdm"));

        for broken in [
            "user-db:user\n",
            "",
            "system-db:distro\nfile-db:/usr/share/gdm/greeter-dconf-defaults\n",
            "system-db:gdmx\n",
            "#system-db:gdm\n",
        ] {
            assert!(
                !profile_reads_system_db(broken, "gdm"),
                "{broken:?} must not count as reading system-db:gdm"
            );
        }
    }

    #[test]
    fn dconf_booleans_are_read_strictly() {
        assert_eq!(dconf_bool("true"), Some(true));
        assert_eq!(dconf_bool("false"), Some(false));
        for unset in ["", "@as []", "nothing to read", "True"] {
            assert_eq!(dconf_bool(unset), None, "{unset:?} is not a boolean");
        }
    }

    #[test]
    fn a_greeter_extension_list_is_matched_exactly() {
        let uuid = "gaze@gundulabs.com";
        assert!(extensions_include("['gaze@gundulabs.com']", uuid));
        assert!(extensions_include(
            "['dash-to-dock@micxgx.gmail.com', 'gaze@gundulabs.com']",
            uuid
        ));
        assert!(!extensions_include("@as []", uuid));
        assert!(!extensions_include("['other@example.com']", uuid));
        assert!(
            !extensions_include("['gaze-clock-diag@gundulabs.com']", uuid),
            "a different extension sharing the domain must not match"
        );
    }

    #[test]
    fn an_unreadable_module_store_is_never_reported_as_a_missing_policy() {
        let reported = |policy| {
            let mut report = Report::default();
            report_gdm_camera_policy(&mut report, policy);
            let check = report
                .checks
                .into_iter()
                .find(|check| check.name == "GDM camera SELinux policy")
                .expect("the SELinux check always reports once it runs");
            (check.level, check.message, check.fix.unwrap_or_default())
        };

        let (level, _, _) = reported(ModuleState::Loaded);
        assert_eq!(level, Level::Pass);

        let (level, _, _) = reported(ModuleState::NotLoaded);
        assert_eq!(
            level,
            Level::Error,
            "a module store we read and found empty is a real failure"
        );

        let (level, message, fix) = reported(ModuleState::NeedsRoot);
        assert_eq!(level, Level::Warning);
        assert!(
            message.contains("without root"),
            "an unprivileged run must say what it could not see: {message}"
        );
        assert!(
            !message.contains("is not loaded"),
            "an unchecked module must not be reported as absent: {message}"
        );
        assert!(
            fix.contains("sudo gaze doctor"),
            "the fix is to re-run as root, not to load the module: {fix}"
        );
        assert!(
            !fix.contains("semodule -i"),
            "loading a module that may already be there is not the remedy: {fix}"
        );

        let (level, _, _) = reported(ModuleState::Unverifiable("broken".into()));
        assert_eq!(level, Level::Warning);
    }

    #[test]
    fn extension_installed_needs_the_metadata_file() {
        let root =
            std::env::temp_dir().join(format!("gaze-doctor-installed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let share = root.join("usr/share");
        let extension = share
            .join("gnome-shell")
            .join("extensions")
            .join(GNOME_EXTENSION_ID);
        fs::create_dir_all(&extension).unwrap();

        let dirs = vec![share.clone()];
        assert!(
            !extension_installed_in(&dirs),
            "a leftover directory is not an installed extension"
        );

        fs::write(extension.join("metadata.json"), b"{}").unwrap();
        assert!(extension_installed_in(&dirs));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extension_schema_dir_finds_a_compiled_schema_in_the_extension() {
        let root = std::env::temp_dir().join(format!("gaze-doctor-schema-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let fhs = root.join("usr/share");
        let nix = root.join("nix/share");
        let extension = nix
            .join("gnome-shell")
            .join("extensions")
            .join(GNOME_EXTENSION_ID);
        fs::create_dir_all(fhs.join("gnome-shell/extensions").join(GNOME_EXTENSION_ID)).unwrap();
        fs::create_dir_all(extension.join("schemas")).unwrap();

        let dirs = vec![fhs.clone(), nix.clone()];
        assert_eq!(
            extension_schema_dir_in(&dirs),
            None,
            "an extension directory without a compiled schema must not be used"
        );

        fs::write(extension.join("schemas/gschemas.compiled"), b"").unwrap();
        assert_eq!(
            extension_schema_dir_in(&dirs),
            Some(extension.join("schemas"))
        );

        let _ = fs::remove_dir_all(&root);
    }
}
