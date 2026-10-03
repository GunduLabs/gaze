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
