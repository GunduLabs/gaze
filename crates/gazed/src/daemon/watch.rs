// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const RESUME_SEEN_MARKER: &str = "/run/gaze/resume-seen";

pub fn resume_seen_marker_exists() -> bool {
    std::path::Path::new(RESUME_SEEN_MARKER).exists()
}

fn persist_resume_seen_marker() -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(RESUME_SEEN_MARKER).parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(RESUME_SEEN_MARKER)?;
    Ok(())
}

pub(super) async fn prepare_for_sleep_stream(
    conn: &zbus::Connection,
) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")
        .interface("org.freedesktop.login1.Manager")
        .member("PrepareForSleep")
        .path("/org/freedesktop/login1")
        .build()?;
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

async fn preparing_for_sleep(conn: &zbus::Connection) -> zbus::Result<bool> {
    zbus::Proxy::new(
        conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await?
    .get_property("PreparingForSleep")
    .await
}

pub async fn watch_resume(
    conn: zbus::Connection,
    resume_pending: Arc<AtomicBool>,
    resume_seen: Arc<AtomicBool>,
    sleeping: tokio::sync::watch::Sender<bool>,
) -> zbus::Result<()> {
    let mut stream = prepare_for_sleep_stream(&conn).await?;

    if let Ok(true) = preparing_for_sleep(&conn).await {
        sleeping.send_replace(true);
    }

    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => return Err(e),
            None => {
                return Err(zbus::Error::Failure(
                    "PrepareForSleep stream ended".to_string(),
                ));
            }
        };
        match msg.body().deserialize::<bool>() {
            Ok(true) => {
                sleeping.send_replace(true);
            }
            Ok(false) => {
                if let Err(e) = persist_resume_seen_marker() {
                    warn!("Failed to persist resume marker: {e}");
                }
                sleeping.send_modify(|state| {
                    resume_pending.store(true, Ordering::SeqCst);
                    resume_seen.store(true, Ordering::SeqCst);
                    *state = false;
                });
            }
            Err(_) => {}
        }
    }
}

/// Clear pending under the watch lock without erasing a newer resume.
pub(super) fn acknowledge_resume(
    sleeping: &mut tokio::sync::watch::Receiver<bool>,
    resume_pending: &AtomicBool,
) -> bool {
    let state = sleeping.borrow_and_update();
    if state.has_changed() || *state {
        return false;
    }
    resume_pending.store(false, Ordering::SeqCst);
    true
}

pub(super) async fn sleep_reaches(
    sleeping: &mut tokio::sync::watch::Receiver<bool>,
    state: bool,
) -> bool {
    sleeping.wait_for(|s| *s == state).await.is_ok()
}

pub(super) async fn hold_for_sleep(
    sleeping: &mut tokio::sync::watch::Receiver<bool>,
    cancel: &mut oneshot::Receiver<()>,
    lid_wait: bool,
) -> SleepHold {
    let already_sleeping = *sleeping.borrow();
    if lid_wait && !already_sleeping {
        info!("Lid is closed, waiting to see whether the system suspends");
        tokio::select! {
            biased;
            _ = &mut *cancel => return SleepHold::Cancelled,
            started = tokio::time::timeout(LID_SLEEP_WAIT, sleep_reaches(sleeping, true)) => {
                if !matches!(started, Ok(true)) {
                    return SleepHold::LidClosed;
                }
            }
        }
    }

    let sleeping_now = *sleeping.borrow();
    if !sleeping_now {
        return SleepHold::Awake;
    }

    info!("System is suspending, holding face auth until resume");
    tokio::select! {
        biased;
        _ = &mut *cancel => SleepHold::Cancelled,
        _ = sleep_reaches(sleeping, false) => {
            info!("Resumed, restarting face auth");
            SleepHold::Resumed
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SleepHold {
    Awake,
    Resumed,
    Cancelled,
    LidClosed,
}

/// Subscribe to NameOwnerChanged, resolving only once the match rule is installed. Call it
/// before requesting the well-known name, or a sender vanishing in between strands the claim.
pub async fn subscribe_claim_owners(
    conn: &zbus::Connection,
) -> zbus::Result<fdo::NameOwnerChangedStream> {
    fdo::DBusProxy::new(conn)
        .await?
        .receive_name_owner_changed()
        .await
}

/// Release claims and forget registered PAM services when their sender vanishes.
/// Uses one subscription for the daemon's lifetime.
pub async fn watch_claim_owner(
    mut stream: fdo::NameOwnerChangedStream,
    claim_state: ClaimStateHandle,
    active_cancel: ActiveCancelHandle,
    pam_internal: PamInternalSets,
) -> zbus::Result<()> {
    while let Some(signal) = stream.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };

        let name = args.name().as_str();
        if args.new_owner().is_none()
            && forget_pam_internal_sender(&mut *pam_internal.lock().await, name)
        {
            info!(sender = %name, "Sender vanished, dropping its internal PAM services");
        }
        let epoch = {
            let state = claim_state.lock().await;
            match &*state {
                Some(claim)
                    if is_vanish_of(
                        name,
                        args.new_owner().as_ref().map(|o| o.as_str()),
                        &claim.sender,
                    ) =>
                {
                    Some(claim.epoch)
                }
                _ => None,
            }
        };
        let Some(epoch) = epoch else {
            continue;
        };

        let name = name.to_string();
        if release_claim_epoch(&claim_state, &active_cancel, epoch).await {
            info!(sender = %name, "Sender vanished, auto-releasing claim");
        }
    }

    Err(zbus::Error::Failure(
        "NameOwnerChanged stream ended; claims will only be released on timeout".to_string(),
    ))
}

pub(super) async fn session_properties_stream(
    conn: &zbus::Connection,
) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")
        .interface("org.freedesktop.DBus.Properties")
        .member("PropertiesChanged")
        .path_namespace(gaze_core::dbus::LOGIN_SESSION_PATH_PREFIX)
        .build()?;
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

pub(super) fn locked_hint_from_changed(body: &zbus::message::Body) -> Option<bool> {
    let (interface, changed, _invalidated): (
        String,
        std::collections::HashMap<String, zbus::Variant>,
        Vec<String>,
    ) = body.deserialize().ok()?;

    locked_hint_from_parts(&interface, &changed)
}

/// Parse boolean `LockedHint` changes from logind Session signals.
fn locked_hint_from_parts(
    interface: &str,
    changed: &std::collections::HashMap<String, zbus::Variant>,
) -> Option<bool> {
    if interface != "org.freedesktop.login1.Session" {
        return None;
    }

    match changed.get("LockedHint")? {
        zbus::Variant::Bool(locked) => Some(*locked),
        _ => None,
    }
}

/// Records when each session locks, so the start delay can be measured from it.
pub async fn watch_session_locks(
    conn: zbus::Connection,
    lock_epochs: LockEpochs,
) -> zbus::Result<()> {
    let mut stream = session_properties_stream(&conn).await?;

    loop {
        let msg = match stream.next().await {
            Some(Ok(msg)) => msg,
            Some(Err(e)) => return Err(e),
            None => {
                return Err(zbus::Error::Failure(
                    "session LockedHint stream ended".to_string(),
                ));
            }
        };
        let Some(path) = msg.header().path().map(|p| p.to_string()) else {
            continue;
        };
        let Some(locked) = locked_hint_from_changed(&msg.body()) else {
            continue;
        };

        let live = gaze_core::dbus::session_paths_on(&conn).await.ok();

        let mut epochs = lock_epochs.lock().await;
        if let Some(live) = live {
            epochs.retain(|session, _| live.iter().any(|path| path == session));
        }
        if locked {
            epochs.entry(path).or_insert_with(std::time::Instant::now);
        } else {
            epochs.remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_with(
        value: Option<zbus::Variant>,
    ) -> std::collections::HashMap<String, zbus::Variant> {
        let mut map = std::collections::HashMap::new();
        if let Some(value) = value {
            map.insert("LockedHint".to_string(), value);
        }
        map
    }

    #[test]
    fn resume_markers_survive_stop_start_until_reboot_in_both_units() {
        let unit = include_str!("../../../../packaging/config/gazed.service");
        let nix = include_str!("../../../../packaging/nix/nixos-module.nix");
        assert!(
            unit.lines()
                .any(|line| line == "RuntimeDirectoryPreserve=yes")
        );
        assert!(nix.contains("RuntimeDirectoryPreserve = \"yes\";"));
    }

    #[test]
    fn a_new_resume_is_not_cleared_before_its_grace_period() {
        let (tx, mut sleeping) = tokio::sync::watch::channel(false);
        let pending = AtomicBool::new(false);
        tx.send_replace(true);
        tx.send_modify(|state| {
            pending.store(true, Ordering::SeqCst);
            *state = false;
        });
        assert!(!acknowledge_resume(&mut sleeping, &pending));
        assert!(pending.load(Ordering::SeqCst));
        assert!(acknowledge_resume(&mut sleeping, &pending));
        assert!(!pending.load(Ordering::SeqCst));
    }

    #[test]
    fn a_sleeping_system_cannot_acknowledge_resume() {
        let (_tx, mut sleeping) = tokio::sync::watch::channel(true);
        let pending = AtomicBool::new(true);
        assert!(!acknowledge_resume(&mut sleeping, &pending));
        assert!(pending.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn an_awake_system_is_not_held() {
        let (_tx, mut sleeping) = tokio::sync::watch::channel(false);
        let (_cancel_tx, mut cancel) = oneshot::channel::<()>();
        assert_eq!(
            hold_for_sleep(&mut sleeping, &mut cancel, false).await,
            SleepHold::Awake
        );
    }

    #[tokio::test]
    async fn a_suspending_system_holds_auth_until_resume() {
        let (tx, mut sleeping) = tokio::sync::watch::channel(true);
        let (_cancel_tx, mut cancel) = oneshot::channel::<()>();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            tx.send_replace(false);
        });
        assert_eq!(
            hold_for_sleep(&mut sleeping, &mut cancel, false).await,
            SleepHold::Resumed
        );
    }

    #[tokio::test]
    async fn a_closed_lid_followed_by_suspend_is_held_until_resume() {
        let (tx, mut sleeping) = tokio::sync::watch::channel(false);
        let (_cancel_tx, mut cancel) = oneshot::channel::<()>();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            tx.send_replace(true);
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send_replace(false);
        });
        assert_eq!(
            hold_for_sleep(&mut sleeping, &mut cancel, true).await,
            SleepHold::Resumed
        );
    }

    #[tokio::test]
    async fn cancelling_ends_a_held_attempt() {
        let (_tx, mut sleeping) = tokio::sync::watch::channel(true);
        let (cancel_tx, mut cancel) = oneshot::channel::<()>();
        cancel_tx.send(()).unwrap();
        assert_eq!(
            hold_for_sleep(&mut sleeping, &mut cancel, true).await,
            SleepHold::Cancelled
        );
    }

    #[test]
    fn session_locked_hint_true_and_false_both_parse() {
        for locked in [true, false] {
            assert_eq!(
                locked_hint_from_parts(
                    "org.freedesktop.login1.Session",
                    &changed_with(Some(zbus::Variant::Bool(locked))),
                ),
                Some(locked)
            );
        }
    }

    #[test]
    fn unrelated_interfaces_are_ignored() {
        for interface in [
            "",
            "org.freedesktop.DBus.Properties",
            "org.freedesktop.login1.Manager",
            "org.freedesktop.login1.session",
        ] {
            assert_eq!(
                locked_hint_from_parts(interface, &changed_with(Some(zbus::Variant::Bool(true))),),
                None,
                "interface {interface:?} must not yield a lock state"
            );
        }
    }

    #[test]
    fn missing_or_non_boolean_locked_hint_is_ignored() {
        let session = "org.freedesktop.login1.Session";
        assert_eq!(locked_hint_from_parts(session, &changed_with(None)), None);
        assert_eq!(
            locked_hint_from_parts(
                session,
                &changed_with(Some(zbus::Variant::Str("true".into()))),
            ),
            None,
            "a string LockedHint must not be trusted"
        );
        assert_eq!(
            locked_hint_from_parts(session, &changed_with(Some(zbus::Variant::U32(1))),),
            None
        );
    }

    #[test]
    fn other_properties_do_not_trigger_lock_tracking() {
        let mut map = std::collections::HashMap::new();
        map.insert("Active".to_string(), zbus::Variant::Bool(true));
        assert_eq!(
            locked_hint_from_parts("org.freedesktop.login1.Session", &map),
            None
        );
    }
}
