// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

impl AuthDaemon {
    pub fn normalize_pam_service(service: &str) -> String {
        let trimmed = service.trim();
        std::path::Path::new(trimmed)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(trimmed)
            .to_string()
    }

    pub(super) async fn current_config(&self) -> Config {
        let mut last_good = self.last_good_config.lock().await;
        resolve_config(Config::load_from(CONFIG_PATH), &mut last_good)
    }

    pub(super) fn map_user_db_error(err: UserDbError) -> fdo::Error {
        let message = err.to_string();
        match err {
            UserDbError::UserNotFound(_) | UserDbError::FaceNotFound(_) => {
                fdo::Error::FileNotFound(message)
            }
            UserDbError::FaceExists(_) => fdo::Error::FileExists(message),
            UserDbError::InvalidName(_) => fdo::Error::InvalidArgs(message),
            UserDbError::Io(_) => fdo::Error::Failed(message),
        }
    }

    pub(super) fn may_query_extension(caller_uid: u32, target_uid: u32) -> bool {
        caller_uid == 0 || caller_uid == target_uid
    }

    pub(super) async fn emit_effective_face_status(
        ctxt: &SignalEmitter<'_>,
        last_emitted_status: &mut Option<CaptureStatus>,
        rgb_status: CaptureStatus,
        ir_status: CaptureStatus,
    ) {
        let effective_status = if rgb_status.priority() >= ir_status.priority() {
            rgb_status
        } else {
            ir_status
        };
        if last_emitted_status.as_ref() != Some(&effective_status) {
            let _ = Self::face_status(ctxt, effective_status).await;
            *last_emitted_status = Some(effective_status);
        }
    }

    pub(super) fn username_uid(username: &str) -> fdo::Result<u32> {
        UserDatabase::validate_username(username).map_err(Self::map_user_db_error)?;

        let c_username = CString::new(username)
            .map_err(|_| fdo::Error::InvalidArgs("username contains NUL byte".into()))?;
        let mut pwd = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result: *mut libc::passwd = ptr::null_mut();
        let buf_size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        let buf_size = if buf_size > 0 {
            buf_size as usize
        } else {
            16 * 1024
        };
        let mut buf = vec![0u8; buf_size];

        let ret = unsafe {
            libc::getpwnam_r(
                c_username.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };

        if ret != 0 {
            return Err(fdo::Error::Failed(format!(
                "failed to resolve user '{username}'"
            )));
        }
        if result.is_null() {
            return Err(fdo::Error::AccessDenied(format!(
                "unknown user '{username}'"
            )));
        }

        Ok(pwd.pw_uid)
    }

    pub(super) async fn caller_uid(header: &Header<'_>) -> fdo::Result<u32> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;
        dbus_proxy()
            .await?
            .get_connection_unix_user(sender.to_owned().into())
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to get caller uid: {e}")))
    }

    pub(super) async fn caller_pid(header: &Header<'_>) -> fdo::Result<u32> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;
        dbus_proxy()
            .await?
            .get_connection_unix_process_id(sender.to_owned().into())
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to get caller pid: {e}")))
    }

    pub(super) fn environ_has_ssh_marker(environ: &[u8]) -> bool {
        environ.split(|b| *b == 0).any(|entry| {
            (entry.starts_with(b"SSH_CONNECTION=") && entry.len() > b"SSH_CONNECTION=".len())
                || (entry.starts_with(b"SSH_TTY=") && entry.len() > b"SSH_TTY=".len())
        })
    }

    pub(super) fn read_ppid_at(base: &std::path::Path, pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(base.join(pid.to_string()).join("stat")).ok()?;
        // /proc/<pid>/stat encloses comm in parentheses, but comm itself may contain spaces
        // and ')'. Split at the last ')' before counting the state and parent-PID fields.
        let after_comm = stat.rsplit_once(')')?.1;
        let mut fields = after_comm.split_whitespace();
        let _state = fields.next()?;
        fields.next()?.parse::<u32>().ok()
    }

    pub(super) fn proc_is_sshd_at(base: &std::path::Path, pid: u32) -> bool {
        std::fs::read_to_string(base.join(pid.to_string()).join("comm"))
            .map(|comm| {
                let comm = comm.trim();
                comm == "sshd" || comm == "sshd-session"
            })
            .unwrap_or(false)
    }

    pub(super) fn proc_environ_is_ssh_at(base: &std::path::Path, pid: u32) -> bool {
        std::fs::read(base.join(pid.to_string()).join("environ"))
            .map(|env| Self::environ_has_ssh_marker(&env))
            .unwrap_or(false)
    }
    pub(super) fn process_chain_is_ssh_at(base: &std::path::Path, pid: u32) -> bool {
        let mut current = pid;
        for _ in 0..SSH_PROC_CHAIN_MAX_DEPTH {
            if Self::proc_environ_is_ssh_at(base, current) || Self::proc_is_sshd_at(base, current) {
                return true;
            }
            match Self::read_ppid_at(base, current) {
                Some(ppid) if ppid != 0 && ppid != current => current = ppid,
                _ => break,
            }
        }
        false
    }

    pub(super) fn caller_is_ssh_session_at(
        base: &std::path::Path,
        caller_pid: Option<u32>,
    ) -> bool {
        match caller_pid {
            Some(pid) => Self::process_chain_is_ssh_at(base, pid),
            None => true,
        }
    }

    pub(super) fn ssh_session_verdict(
        heuristic_is_ssh: bool,
        session_remote: Option<bool>,
    ) -> bool {
        heuristic_is_ssh || session_remote.unwrap_or(false)
    }

    pub(super) async fn caller_session_is_remote(pid: u32) -> Option<bool> {
        let conn = system_bus().await.ok()?;
        gaze_core::dbus::session_is_remote_on(&conn, pid).await.ok()
    }

    pub(super) fn lid_state_is_closed(state: &str) -> bool {
        state.to_ascii_lowercase().contains("closed")
    }

    pub(super) fn is_lid_closed_at(base: &std::path::Path) -> bool {
        let Ok(entries) = std::fs::read_dir(base) else {
            return false;
        };

        entries.filter_map(Result::ok).any(|entry| {
            std::fs::read_to_string(entry.path().join("state"))
                .map(|state| Self::lid_state_is_closed(&state))
                .unwrap_or(false)
        })
    }

    pub(super) fn upower_lid_closed(present: bool, closed: bool) -> bool {
        present && closed
    }
    pub(super) async fn lid_is_closed_via_upower() -> Option<bool> {
        let conn = system_bus().await.ok()?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower",
            "org.freedesktop.UPower",
        )
        .await
        .ok()?;
        let present: bool = proxy.get_property("LidIsPresent").await.ok()?;
        let closed: bool = proxy.get_property("LidIsClosed").await.ok()?;
        Some(Self::upower_lid_closed(present, closed))
    }

    pub(super) async fn is_lid_closed() -> bool {
        if let Some(closed) = Self::lid_is_closed_via_upower().await {
            return closed;
        }
        Self::is_lid_closed_at(std::path::Path::new("/proc/acpi/button/lid"))
    }

    pub(super) fn resume_gate_blocks(abort_before_first_resume: bool, resume_seen: bool) -> bool {
        abort_before_first_resume && !resume_seen
    }

    pub(super) async fn ensure_auth_not_aborted(&self, header: &Header<'_>) -> fdo::Result<()> {
        let abort_before_first_resume = *self.abort_before_first_resume.lock().await;
        if Self::resume_gate_blocks(
            abort_before_first_resume,
            self.resume_seen.load(Ordering::SeqCst),
        ) {
            warn!("No suspend/resume since boot, aborting face auth");
            return Err(fdo::Error::Failed("no suspend/resume since boot".into()));
        }

        let abort_if_ssh = *self.abort_if_ssh.lock().await;
        if abort_if_ssh {
            let caller_pid = Self::caller_pid(header).await.ok();
            let heuristic_is_ssh =
                Self::caller_is_ssh_session_at(std::path::Path::new("/proc"), caller_pid);
            let session_remote = match caller_pid {
                Some(pid) if !heuristic_is_ssh => Self::caller_session_is_remote(pid).await,
                _ => None,
            };
            if Self::ssh_session_verdict(heuristic_is_ssh, session_remote) {
                warn!(caller_pid, "SSH session detected, aborting face auth");
                return Err(fdo::Error::Failed("SSH session detected".into()));
            }
        }

        let abort_if_lid_closed = *self.abort_if_lid_closed.lock().await;
        if abort_if_lid_closed && Self::is_lid_closed().await {
            warn!("Laptop lid is closed, aborting face auth");
            return Err(fdo::Error::Failed("lid closed".into()));
        }

        Ok(())
    }

    pub(super) async fn ensure_user_access(
        header: &Header<'_>,
        username: &str,
        action_id: &str,
    ) -> fdo::Result<()> {
        let caller_uid = Self::caller_uid(header).await?;
        let target_uid = Self::username_uid(username)?;
        if caller_uid == 0 || caller_uid == target_uid {
            return Ok(());
        }

        Self::ensure_authorized_with(header, action_id, Interaction::Deny).await
    }

    pub(super) fn face_write_needs_authorization(caller_uid: u32) -> bool {
        caller_uid != 0
    }

    pub(super) fn benchmark_needs_authorization(caller_uid: u32) -> bool {
        caller_uid != 0
    }

    pub(super) async fn ensure_face_write_access(
        header: &Header<'_>,
        username: &str,
        action_id: &str,
    ) -> fdo::Result<()> {
        Self::username_uid(username)?;
        if !Self::face_write_needs_authorization(Self::caller_uid(header).await?) {
            return Ok(());
        }

        Self::ensure_authorized(header, action_id).await
    }

    // The GDM greeter asks which login users have faces and cannot answer an
    // interactive polkit challenge. `active` is (uid, is_greeter) for the seat.
    pub(super) fn config_read_allowed(caller_uid: u32, active_uid: Option<u32>) -> bool {
        caller_uid == 0 || active_uid == Some(caller_uid)
    }

    pub(super) async fn ensure_config_read_access(header: &Header<'_>) -> fdo::Result<()> {
        let caller_uid = Self::caller_uid(header).await?;
        let active_uid = active_session_uid_and_class().await.map(|(uid, _)| uid);
        if Self::config_read_allowed(caller_uid, active_uid) {
            return Ok(());
        }
        Err(fdo::Error::AccessDenied(
            "only root or the active session may read the Gaze configuration".into(),
        ))
    }

    pub(super) fn pam_internal_write_allowed(caller_uid: u32, active_uid: Option<u32>) -> bool {
        caller_uid == 0 || active_uid == Some(caller_uid)
    }

    // The PAM module runs as root and cannot say whose dialog it is feeding, so root is answered
    // for the active session, the one whose shell renders the prompt.
    pub(super) fn pam_internal_owner(caller_uid: u32, active_uid: Option<u32>) -> u32 {
        if caller_uid == 0 {
            active_uid.unwrap_or(0)
        } else {
            caller_uid
        }
    }

    pub(super) async fn pam_internal_read_owner(header: &Header<'_>) -> fdo::Result<u32> {
        let caller_uid = Self::caller_uid(header).await?;
        let active_uid = active_session_uid_and_class().await.map(|(uid, _)| uid);
        Ok(Self::pam_internal_owner(caller_uid, active_uid))
    }

    pub(super) async fn pam_internal_write_owner(header: &Header<'_>) -> fdo::Result<u32> {
        let caller_uid = Self::caller_uid(header).await?;
        let active_uid = active_session_uid_and_class().await.map(|(uid, _)| uid);
        if Self::pam_internal_write_allowed(caller_uid, active_uid) {
            return Ok(Self::pam_internal_owner(caller_uid, active_uid));
        }
        Err(fdo::Error::AccessDenied(
            "only root or the active session may modify the PAM internal services list".into(),
        ))
    }

    pub(super) fn user_query_allowed(
        caller_uid: u32,
        target_uid: u32,
        active: Option<(u32, bool)>,
    ) -> bool {
        if caller_uid == 0 || caller_uid == target_uid {
            return true;
        }
        matches!(active, Some((uid, true)) if uid == caller_uid)
    }

    pub(super) async fn ensure_user_query_access(
        header: &Header<'_>,
        username: &str,
        action_id: &str,
    ) -> fdo::Result<()> {
        let caller_uid = Self::caller_uid(header).await?;
        let target_uid = Self::username_uid(username)?;
        let active = active_session_uid_and_class()
            .await
            .map(|(uid, class)| (uid, class == "greeter"));
        if Self::user_query_allowed(caller_uid, target_uid, active) {
            return Ok(());
        }

        Self::ensure_authorized_with(header, action_id, Interaction::Deny).await
    }

    pub(super) fn signal_destination(sender: &str) -> fdo::Result<BusName<'static>> {
        BusName::try_from(sender.to_string())
            .map_err(|e| fdo::Error::Failed(format!("Invalid signal destination: {e}")))
    }

    pub(super) async fn ensure_authorized(header: &Header<'_>, action_id: &str) -> fdo::Result<()> {
        Self::ensure_authorized_with(header, action_id, Interaction::Allow).await
    }

    pub(super) async fn ensure_authorized_with(
        header: &Header<'_>,
        action_id: &str,
        interaction: Interaction,
    ) -> fdo::Result<()> {
        let conn = system_bus().await?;

        let authority = zbus_polkit::policykit1::AuthorityProxy::new(&conn)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to create polkit proxy: {e}")))?;

        let subject = zbus_polkit::policykit1::Subject::new_for_message_header(header)
            .map_err(|e| fdo::Error::Failed(format!("Failed to create polkit subject: {e}")))?;

        let details: HashMap<&str, &str> = HashMap::new();
        let flags = match interaction {
            Interaction::Allow => {
                zbus_polkit::policykit1::CheckAuthorizationFlags::AllowUserInteraction.into()
            }
            Interaction::Deny => Default::default(),
        };

        let result = authority
            .check_authorization(&subject, action_id, &details, flags, "")
            .await
            .map_err(|e| fdo::Error::Failed(format!("PolicyKit CheckAuthorization failed: {e}")))?;

        if !result.is_authorized {
            return Err(fdo::Error::AccessDenied(format!(
                "Authorization denied for action '{action_id}'"
            )));
        }

        Ok(())
    }

    pub(super) async fn check_claim(&self, header: &Header<'_>) -> fdo::Result<ClaimState> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let state = self.claim_state.lock().await;
        if let Some(claim) = &*state {
            if claim.sender == sender {
                return Ok(claim.clone());
            } else {
                return Err(fdo::Error::Failed(
                    "Daemon is claimed by another process".into(),
                ));
            }
        }
        Err(fdo::Error::Failed("Daemon is not claimed".into()))
    }

    // Every capture opens the seat's V4L2 device, so a bystander's camera must never
    // authenticate another user: the target, or a caller vouching for them, has to hold the seat.
    // `active` is (uid, is_greeter) for the active seat; `seat_unoccupied` is its own check.
    pub(super) fn seat_camera_allowed(
        caller_uid: u32,
        target_uid: u32,
        active: Option<(u32, bool)>,
        seat_unoccupied: bool,
    ) -> bool {
        match active {
            Some((active_uid, true)) => caller_uid == 0 || caller_uid == active_uid,
            Some((active_uid, false)) => {
                active_uid == target_uid || (caller_uid != 0 && caller_uid == active_uid)
            }
            // A console login runs before any session exists, so with the seat otherwise empty
            // nobody's ACL can be taken. A failed lookup leaves this false and still refuses.
            None => caller_uid == 0 && seat_unoccupied,
        }
    }

    /// Whether seat0 holds no session that belongs to anyone other than `target_uid`.
    /// A failed enumeration is reported as occupied so the caller fails closed.
    pub(super) async fn seat_is_unoccupied(target_uid: u32) -> bool {
        let uids = match system_bus().await {
            Ok(conn) => gaze_core::dbus::seat0_session_uids_on(&conn).await,
            Err(e) => Err(anyhow::anyhow!(e)),
        };
        match uids {
            Ok(uids) => uids.iter().all(|uid| *uid == target_uid),
            Err(_) => false,
        }
    }

    pub(super) async fn seat_camera_available(caller_uid: u32, target_uid: u32) -> bool {
        let lookup = match system_bus().await {
            Ok(conn) => gaze_core::dbus::active_session_lookup_on(&conn).await,
            Err(e) => Err(anyhow::anyhow!(e)),
        };
        let (active, seat_idle) = match lookup {
            Ok(Some(session)) => (Some((session.uid, session.class == "greeter")), false),
            Ok(None) => (None, true),
            Err(_) => (None, false),
        };
        let seat_unoccupied = seat_idle && Self::seat_is_unoccupied(target_uid).await;
        Self::seat_camera_allowed(caller_uid, target_uid, active, seat_unoccupied)
    }

    pub(super) async fn cancel_active_tasks(&self) {
        let mut cancel = self.active_cancel.lock().await;
        if let Some(sender) = cancel.take() {
            let _ = sender.send(());
        }
    }

    /// `gdm-face` serves the greeter as well as the lock screen.
    pub(super) fn classify_surface(
        pam_service: Option<&str>,
        active_session: Option<&gaze_core::dbus::ActiveSession>,
    ) -> gaze_core::config::AuthSurface {
        let surface = gaze_core::config::classify_pam_service(pam_service);
        if surface == gaze_core::config::AuthSurface::ScreenLock
            && active_session.is_some_and(|session| session.is_greeter())
        {
            return gaze_core::config::AuthSurface::Login;
        }
        surface
    }

    pub(super) async fn lock_elapsed_ms(
        &self,
        active_session: Option<&gaze_core::dbus::ActiveSession>,
    ) -> Option<u64> {
        let session = active_session?;
        let epochs = self.lock_epochs.lock().await;
        let started = epochs.get(&session.path)?;
        Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_core::config::AuthSurface;
    use gaze_core::dbus::ActiveSession;
    use std::sync::Arc;

    fn session(class: &str) -> ActiveSession {
        ActiveSession {
            uid: 1000,
            class: class.to_string(),
            path: "/org/freedesktop/login1/session/_32".to_string(),
        }
    }

    #[test]
    fn the_resume_gate_only_blocks_before_the_first_resume() {
        assert!(AuthDaemon::resume_gate_blocks(true, false));
        assert!(!AuthDaemon::resume_gate_blocks(true, true));
        assert!(!AuthDaemon::resume_gate_blocks(false, false));
        assert!(!AuthDaemon::resume_gate_blocks(false, true));
    }

    #[test]
    fn gdm_face_on_the_greeter_is_a_login_not_a_screen_lock() {
        assert_eq!(
            AuthDaemon::classify_surface(Some("gdm-face"), Some(&session("greeter"))),
            AuthSurface::Login
        );
        assert_eq!(
            AuthDaemon::classify_surface(Some("gdm-face"), Some(&session("user"))),
            AuthSurface::ScreenLock
        );
        assert_eq!(
            AuthDaemon::classify_surface(Some("gdm-face"), None),
            AuthSurface::ScreenLock
        );
    }

    #[test]
    fn a_greeter_session_does_not_reclassify_elevation() {
        assert_eq!(
            AuthDaemon::classify_surface(Some("sudo"), Some(&session("greeter"))),
            AuthSurface::Elevation
        );
    }

    #[test]
    fn root_and_the_active_session_may_read_the_config() {
        assert!(AuthDaemon::config_read_allowed(0, Some(1000)));
        assert!(AuthDaemon::config_read_allowed(0, None));
        assert!(AuthDaemon::config_read_allowed(1000, Some(1000)));
    }

    #[test]
    fn other_local_users_may_not_read_the_config() {
        assert!(!AuthDaemon::config_read_allowed(1001, Some(1000)));
        assert!(!AuthDaemon::config_read_allowed(1000, None));
        assert!(!AuthDaemon::config_read_allowed(65534, Some(1000)));
    }

    #[test]
    fn root_and_the_active_session_may_modify_pam_internal() {
        assert!(AuthDaemon::pam_internal_write_allowed(0, Some(1000)));
        assert!(AuthDaemon::pam_internal_write_allowed(0, None));
        assert!(AuthDaemon::pam_internal_write_allowed(1000, Some(1000)));
    }

    #[test]
    fn pam_internal_is_kept_per_session_user() {
        assert_eq!(AuthDaemon::pam_internal_owner(1000, Some(1000)), 1000);
        // Another user's registration never leaks into the active session's lookup.
        assert_eq!(AuthDaemon::pam_internal_owner(1001, Some(1000)), 1001);
        assert_eq!(AuthDaemon::pam_internal_owner(0, Some(1001)), 1001);
        assert_eq!(AuthDaemon::pam_internal_owner(0, None), 0);
    }

    #[test]
    fn other_local_users_may_not_modify_pam_internal() {
        assert!(!AuthDaemon::pam_internal_write_allowed(1001, Some(1000)));
        assert!(!AuthDaemon::pam_internal_write_allowed(1000, None));
        assert!(!AuthDaemon::pam_internal_write_allowed(65534, Some(1000)));
    }

    #[test]
    fn ssh_marker_detection_requires_non_empty_values() {
        assert!(AuthDaemon::environ_has_ssh_marker(
            b"PATH=/usr/bin\0SSH_CONNECTION=1.2.3.4 1 5.6.7.8 22\0"
        ));
        assert!(AuthDaemon::environ_has_ssh_marker(
            b"SSH_TTY=/dev/pts/3\0USER=alice\0"
        ));
        assert!(!AuthDaemon::environ_has_ssh_marker(
            b"SSH_CONNECTION=\0SSH_TTY=\0"
        ));
        assert!(!AuthDaemon::environ_has_ssh_marker(b"USER=alice\0"));
    }

    #[test]
    fn lid_state_detection_is_case_insensitive() {
        assert!(AuthDaemon::lid_state_is_closed("state:      closed\n"));
        assert!(AuthDaemon::lid_state_is_closed("State: CLOSED\n"));
        assert!(!AuthDaemon::lid_state_is_closed("state:      open\n"));
    }

    #[test]
    fn upower_lid_closed_requires_present_and_closed() {
        assert!(AuthDaemon::upower_lid_closed(true, true));
        // A machine without a lid (e.g. a desktop) is never "closed".
        assert!(!AuthDaemon::upower_lid_closed(true, false));
        assert!(!AuthDaemon::upower_lid_closed(false, true));
        assert!(!AuthDaemon::upower_lid_closed(false, false));
    }

    struct FakeProc {
        root: std::path::PathBuf,
    }

    impl FakeProc {
        fn new(name: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "gaze-proc-test-{}-{}-{name}",
                std::process::id(),
                unique
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn add(&self, pid: u32, ppid: u32, comm: &str, environ: &[u8]) {
            let dir = self.root.join(pid.to_string());
            // Embed parens/spaces in comm to exercise the stat parser.
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("stat"),
                format!("{pid} ({comm}) S {ppid} 1 1 0 -1 0\n"),
            )
            .unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            std::fs::write(dir.join("environ"), environ).unwrap();
        }

        fn root(&self) -> &std::path::Path {
            &self.root
        }
    }

    impl Drop for FakeProc {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn read_ppid_parses_stat_with_parenthesised_comm() {
        let proc = FakeProc::new("ppid");
        proc.add(42, 7, "weird (name)", b"");
        assert_eq!(AuthDaemon::read_ppid_at(proc.root(), 42), Some(7));
        assert_eq!(AuthDaemon::read_ppid_at(proc.root(), 999), None);
    }

    #[test]
    fn ssh_detected_via_ancestor_environ_marker() {
        let proc = FakeProc::new("ancestor-env");
        proc.add(1000, 900, "sshd", b"SSH_CONNECTION=1.2.3.4 5 6.7.8.9 22\0");
        proc.add(1001, 1000, "sudo", b"USER=alice\0");
        proc.add(1002, 1001, "unix_chkpwd", b"USER=alice\0");

        assert!(AuthDaemon::process_chain_is_ssh_at(proc.root(), 1002));
    }

    #[test]
    fn ssh_detected_via_ancestor_comm_when_environ_is_bare() {
        let proc = FakeProc::new("ancestor-comm");
        proc.add(2000, 1, "sshd-session", b"PATH=/usr/bin\0");
        proc.add(2001, 2000, "bash", b"PATH=/usr/bin\0");
        proc.add(2002, 2001, "sudo", b"PATH=/usr/bin\0");

        assert!(AuthDaemon::process_chain_is_ssh_at(proc.root(), 2002));
    }

    #[test]
    fn local_session_chain_is_not_flagged_as_ssh() {
        let proc = FakeProc::new("local");
        proc.add(3000, 1, "systemd", b"PATH=/usr/bin\0");
        proc.add(3001, 3000, "gdm-session-wor", b"PATH=/usr/bin\0");
        proc.add(3002, 3001, "sudo", b"USER=alice\0");

        assert!(!AuthDaemon::process_chain_is_ssh_at(proc.root(), 3002));
    }

    #[test]
    fn unresolved_caller_pid_fails_closed_as_ssh() {
        let proc = FakeProc::new("unresolved-pid");
        assert!(AuthDaemon::caller_is_ssh_session_at(proc.root(), None));
    }

    #[test]
    fn resolved_local_caller_is_not_flagged_as_ssh() {
        let proc = FakeProc::new("resolved-local");
        proc.add(6000, 1, "systemd", b"PATH=/usr/bin\0");
        proc.add(6001, 6000, "sudo", b"USER=alice\0");
        assert!(!AuthDaemon::caller_is_ssh_session_at(
            proc.root(),
            Some(6001)
        ));
    }

    #[test]
    fn detached_scrubbed_process_escapes_environ_ancestry_check() {
        let proc = FakeProc::new("detached");
        proc.add(1, 0, "systemd", b"PATH=/usr/bin\0");
        proc.add(5000, 1, "gaze", b"PATH=/usr/bin\0");
        assert!(!AuthDaemon::process_chain_is_ssh_at(proc.root(), 5000));
        assert!(!AuthDaemon::caller_is_ssh_session_at(
            proc.root(),
            Some(5000)
        ));
    }

    #[test]
    fn ssh_verdict_combines_heuristic_with_logind_remote() {
        assert!(AuthDaemon::ssh_session_verdict(true, None));
        assert!(AuthDaemon::ssh_session_verdict(true, Some(false)));
        assert!(AuthDaemon::ssh_session_verdict(false, Some(true)));
        assert!(!AuthDaemon::ssh_session_verdict(false, Some(false)));
        assert!(!AuthDaemon::ssh_session_verdict(false, None));
    }

    #[test]
    fn process_chain_walk_terminates_on_self_referential_ppid() {
        let proc = FakeProc::new("cycle");
        proc.add(4000, 4000, "bash", b"USER=alice\0");
        assert!(!AuthDaemon::process_chain_is_ssh_at(proc.root(), 4000));
    }

    #[test]
    fn camera_allows_a_root_caller_for_the_user_holding_the_seat() {
        assert!(AuthDaemon::seat_camera_allowed(
            0,
            1001,
            Some((1001, false)),
            false
        ));
    }

    #[test]
    fn camera_refuses_bystander_session_for_root_caller() {
        // su victim from the attacker's seat, whether or not the victim is logged in elsewhere.
        assert!(!AuthDaemon::seat_camera_allowed(
            0,
            1001,
            Some((1000, false)),
            false
        ));
        // A failed logind lookup leaves the seat state unknown, so still refuse.
        assert!(!AuthDaemon::seat_camera_allowed(0, 1001, None, false));
    }

    #[test]
    fn camera_uses_the_seat_device_at_a_console_login_prompt() {
        // `login` on a free VT: no session exists yet, so nothing owns the seat camera.
        assert!(AuthDaemon::seat_camera_allowed(0, 1001, None, true));
    }

    #[test]
    fn camera_refuses_the_seat_device_while_another_user_holds_the_seat() {
        // logind empties ActiveSession on a switch to a VT with no session, even while another
        // user stays logged in on a background VT. Emptiness alone must not reach the device.
        assert!(!AuthDaemon::seat_camera_allowed(0, 1001, None, false));
    }

    #[test]
    fn camera_denies_the_seat_device_to_unprivileged_callers() {
        // An idle seat is not a licence for a non-root caller to reach the device.
        assert!(!AuthDaemon::seat_camera_allowed(1000, 1001, None, true));
    }

    #[test]
    fn camera_allows_login_greeter_for_root_caller() {
        // GDM login, where the target has no session yet and the active seat is the greeter.
        assert!(AuthDaemon::seat_camera_allowed(
            0,
            1001,
            Some((42, true)),
            false
        ));
    }

    #[test]
    fn camera_answers_the_greeter_probing_for_itself() {
        assert!(AuthDaemon::seat_camera_allowed(
            42,
            42,
            Some((42, true)),
            false
        ));
        assert!(!AuthDaemon::seat_camera_allowed(
            1000,
            1000,
            Some((42, true)),
            false
        ));
    }

    #[test]
    fn camera_allows_a_polkit_approved_caller_holding_the_seat() {
        // Admin (non-root) acting for another user after a polkit check, at their own seat.
        assert!(AuthDaemon::seat_camera_allowed(
            1000,
            1001,
            Some((1000, false)),
            false
        ));
        assert!(!AuthDaemon::seat_camera_allowed(1000, 1001, None, false));
    }

    #[test]
    fn camera_refuses_a_background_session_probing_for_itself() {
        assert!(!AuthDaemon::seat_camera_allowed(
            1002,
            1002,
            Some((1000, false)),
            false
        ));
    }

    #[test]
    fn seat_occupancy_ignores_the_target_and_fails_closed() {
        // Only sessions belonging to somebody else count as occupancy.
        assert!([1001, 1001].iter().all(|uid| *uid == 1001));
        assert!(![1001, 1000].iter().all(|uid| *uid == 1001));
        // An empty seat is unoccupied for any target.
        assert!(Vec::<u32>::new().iter().all(|uid| *uid == 1001));
    }

    #[test]
    fn user_queries_allow_root_self_and_active_greeter_only() {
        assert!(AuthDaemon::user_query_allowed(0, 1000, None));
        assert!(AuthDaemon::user_query_allowed(1000, 1000, None));
        // Active greeter may ask about any login user.
        assert!(AuthDaemon::user_query_allowed(42, 1000, Some((42, true))));
        // Non-greeter or inactive callers still need polkit.
        assert!(!AuthDaemon::user_query_allowed(42, 1000, Some((42, false))));
        assert!(!AuthDaemon::user_query_allowed(
            42,
            1000,
            Some((1000, true))
        ));
        assert!(!AuthDaemon::user_query_allowed(42, 1000, None));
    }

    #[test]
    fn face_writes_need_authorization_even_for_the_owning_user() {
        assert!(AuthDaemon::face_write_needs_authorization(1000));
        assert!(!AuthDaemon::face_write_needs_authorization(0));
    }

    #[test]
    fn benchmarks_need_authorization_for_every_non_root_caller() {
        assert!(AuthDaemon::benchmark_needs_authorization(1000));
        assert!(AuthDaemon::benchmark_needs_authorization(42));
        assert!(!AuthDaemon::benchmark_needs_authorization(0));
    }

    #[test]
    fn only_a_privileged_caller_at_a_greeter_reaches_the_seat_device() {
        // Must never let an unprivileged caller borrow a device for someone else.
        assert!(!AuthDaemon::seat_camera_allowed(
            1000,
            1001,
            Some((42, true)),
            false
        ));
        assert!(!AuthDaemon::seat_camera_allowed(
            0,
            1001,
            Some((1000, false)),
            false
        ));
    }

    #[test]
    fn extension_state_is_visible_only_to_root_or_the_target_user() {
        assert!(AuthDaemon::may_query_extension(0, 1000));
        assert!(AuthDaemon::may_query_extension(1000, 1000));
        assert!(!AuthDaemon::may_query_extension(1001, 1000));
    }

    #[test]
    fn pam_internal_service_normalization() {
        assert_eq!(AuthDaemon::normalize_pam_service("polkit-1"), "polkit-1");
        assert_eq!(
            AuthDaemon::normalize_pam_service("  polkit-1  "),
            "polkit-1"
        );
        assert_eq!(
            AuthDaemon::normalize_pam_service("/etc/pam.d/polkit-1"),
            "polkit-1"
        );
        assert_eq!(
            AuthDaemon::normalize_pam_service("/etc/pam.d/gdm-face"),
            "gdm-face"
        );
        assert_eq!(AuthDaemon::normalize_pam_service(""), "");
    }

    #[test]
    fn pam_internal_set_add_remove_clear_logic() {
        let set = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));

        {
            let mut s = set.lock().unwrap();
            let norm = AuthDaemon::normalize_pam_service("/etc/pam.d/polkit-1");
            if !norm.is_empty() {
                s.insert(norm);
            }
            let norm2 = AuthDaemon::normalize_pam_service("gdm-face");
            if !norm2.is_empty() {
                s.insert(norm2);
            }
        }

        {
            let s = set.lock().unwrap();
            assert!(s.contains("polkit-1"));
            assert!(s.contains("gdm-face"));
            assert_eq!(s.len(), 2);
        }

        {
            let mut s = set.lock().unwrap();
            let norm = AuthDaemon::normalize_pam_service("polkit-1");
            s.remove(&norm);
        }

        {
            let s = set.lock().unwrap();
            assert!(!s.contains("polkit-1"));
            assert!(s.contains("gdm-face"));
            assert_eq!(s.len(), 1);
        }

        {
            let mut s = set.lock().unwrap();
            s.clear();
        }

        {
            let s = set.lock().unwrap();
            assert!(s.is_empty());
        }
    }
}
