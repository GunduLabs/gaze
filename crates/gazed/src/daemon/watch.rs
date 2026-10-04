// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) async fn prepare_for_sleep_stream(
    conn: &zbus::Connection,
) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .interface("org.freedesktop.login1.Manager")?
        .member("PrepareForSleep")?
        .path("/org/freedesktop/login1")?
        .build();
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

pub async fn watch_resume(
    conn: zbus::Connection,
    resume_pending: Arc<AtomicBool>,
    resume_seen: Arc<AtomicBool>,
) {
    let mut stream = match prepare_for_sleep_stream(&conn).await {
        Ok(stream) => stream,
        Err(e) => {
            warn!("Failed to subscribe to PrepareForSleep, resume handling disabled: {e}");
            return;
        }
    };

    while let Some(Ok(msg)) = stream.next().await {
        if let Ok(false) = msg.body().deserialize::<bool>() {
            resume_pending.store(true, Ordering::SeqCst);
            resume_seen.store(true, Ordering::SeqCst);
        }
    }
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

/// Release the active claim as soon as its owning D-Bus name loses its owner. One subscription
/// for the daemon's lifetime, so no task or signal receiver is left behind per claim.
pub async fn watch_claim_owner(
    mut stream: fdo::NameOwnerChangedStream,
    claim_state: ClaimStateHandle,
    active_cancel: ActiveCancelHandle,
) {
    while let Some(signal) = stream.next().await {
        let Ok(args) = signal.args() else {
            continue;
        };

        let name = args.name().as_str();
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

    error!("NameOwnerChanged stream ended; claims will only be released on timeout");
}

pub(super) async fn session_properties_stream(
    conn: &zbus::Connection,
) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path_namespace(gaze_core::dbus::LOGIN_SESSION_PATH_PREFIX)?
        .build();
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

pub(super) fn locked_hint_from_changed(body: &zbus::message::Body) -> Option<bool> {
    let (interface, changed, _invalidated): (
        String,
        std::collections::HashMap<String, zbus::zvariant::Value>,
        Vec<String>,
    ) = body.deserialize().ok()?;

    locked_hint_from_parts(&interface, &changed)
}

/// Pure core of [`locked_hint_from_changed`]: the only signals that matter are
/// `org.freedesktop.login1.Session` ones carrying a boolean `LockedHint`.
/// Split out so it can be unit-tested without synthesising D-Bus bodies.
fn locked_hint_from_parts(
    interface: &str,
    changed: &std::collections::HashMap<String, zbus::zvariant::Value>,
) -> Option<bool> {
    if interface != "org.freedesktop.login1.Session" {
        return None;
    }

    match changed.get("LockedHint")? {
        zbus::zvariant::Value::Bool(locked) => Some(*locked),
        _ => None,
    }
}

/// Records when each session locks, so the start delay can be measured from it.
pub async fn watch_session_locks(conn: zbus::Connection, lock_epochs: LockEpochs) {
    let mut stream = match session_properties_stream(&conn).await {
        Ok(stream) => stream,
        Err(e) => {
            warn!(
                "Failed to subscribe to session LockedHint, start delay will apply per auth: {e}"
            );
            return;
        }
    };

    while let Some(Ok(msg)) = stream.next().await {
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

    fn changed_with(value: Option<zbus::zvariant::Value>) -> std::collections::HashMap<String, zbus::zvariant::Value> {
        let mut map = std::collections::HashMap::new();
        if let Some(value) = value {
            map.insert("LockedHint".to_string(), value);
        }
        map
    }

    #[test]
    fn session_locked_hint_true_and_false_both_parse() {
        for locked in [true, false] {
            assert_eq!(
                locked_hint_from_parts(
                    "org.freedesktop.login1.Session",
                    &changed_with(Some(zbus::zvariant::Value::Bool(locked))),
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
                locked_hint_from_parts(
                    interface,
                    &changed_with(Some(zbus::zvariant::Value::Bool(true))),
                ),
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
                &changed_with(Some(zbus::zvariant::Value::Str("true".into()))),
            ),
            None,
            "a string LockedHint must not be trusted"
        );
        assert_eq!(
            locked_hint_from_parts(
                session,
                &changed_with(Some(zbus::zvariant::Value::U32(1))),
            ),
            None
        );
    }

    #[test]
    fn other_properties_do_not_trigger_lock_tracking() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "Active".to_string(),
            zbus::zvariant::Value::Bool(true),
        );
        assert_eq!(
            locked_hint_from_parts("org.freedesktop.login1.Session", &map),
            None
        );
    }
}
