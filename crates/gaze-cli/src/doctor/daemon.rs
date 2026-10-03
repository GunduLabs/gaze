// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DaemonCredentials {
    pub(super) uid: u32,
    pub(super) gids: Vec<u32>,
    pub(super) dac_override: bool,
}

pub(super) fn daemon_credentials() -> Option<DaemonCredentials> {
    let (ok, text) = command_output(
        "systemctl",
        &[
            "show",
            "gazed",
            "-p",
            "LoadState",
            "-p",
            "User",
            "-p",
            "SupplementaryGroups",
            "-p",
            "CapabilityBoundingSet",
        ],
    )
    .ok()?;
    if !ok {
        return None;
    }
    parse_daemon_credentials(&text)
}

pub(super) fn parse_daemon_credentials(text: &str) -> Option<DaemonCredentials> {
    let mut load_state = "";
    let mut user = "";
    let mut groups = "";
    let mut capabilities = "";
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "LoadState" => load_state = value.trim(),
            "User" => user = value.trim(),
            "SupplementaryGroups" => groups = value.trim(),
            "CapabilityBoundingSet" => capabilities = value.trim(),
            _ => {}
        }
    }

    if load_state != "loaded" {
        return None;
    }
    if !(user.is_empty() || user == "root" || user == "0") {
        return None;
    }

    let mut gids = vec![0];
    gids.extend(groups.split_whitespace().filter_map(group_gid));
    Some(DaemonCredentials {
        uid: 0,
        gids,
        dac_override: capabilities
            .split_whitespace()
            .any(|capability| capability.eq_ignore_ascii_case("cap_dac_override")),
    })
}

pub(super) fn node_openable(
    uid: u32,
    gid: u32,
    mode: u32,
    credentials: &DaemonCredentials,
) -> bool {
    if credentials.dac_override {
        return true;
    }
    let class = if uid == credentials.uid {
        0o600
    } else if credentials.gids.contains(&gid) {
        0o060
    } else {
        0o006
    };
    mode & class == class
}

pub(super) fn group_gid(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    let entry = unsafe { libc::getgrnam(name.as_ptr()) };
    if entry.is_null() {
        return None;
    }
    Some(unsafe { (*entry).gr_gid })
}

pub(super) fn user_uid(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    let entry = unsafe { libc::getpwnam(name.as_ptr()) };
    if entry.is_null() {
        return None;
    }
    Some(unsafe { (*entry).pw_uid })
}

pub(super) fn user_name(uid: u32) -> String {
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() {
        return uid.to_string();
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).pw_name) };
    name.to_str().map(str::to_owned).unwrap_or(uid.to_string())
}

pub(super) fn group_name(gid: u32) -> String {
    let entry = unsafe { libc::getgrgid(gid) };
    if entry.is_null() {
        return gid.to_string();
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).gr_name) };
    name.to_str().map(str::to_owned).unwrap_or(gid.to_string())
}

pub(super) async fn read_daemon_config(
    proxy: &GazeProxy<'_>,
    ready_wait: Duration,
) -> zbus::Result<Config> {
    let deadline = Instant::now() + ready_wait;
    loop {
        match proxy.config().await {
            Ok(config) => return Ok(config.into()),
            Err(err) if dbus_is_not_activatable(&err) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Whether `gazed` currently owns its well-known name, retrying for `ready_wait` so a
/// daemon still downloading models is not mistaken for one that will never appear.
/// Connecting to the system bus succeeds regardless, so only this distinguishes the two.
pub(super) async fn gaze_name_has_owner(proxy: &GazeProxy<'_>, ready_wait: Duration) -> bool {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(proxy.inner().connection()).await else {
        return false;
    };
    let Ok(name) = zbus::names::BusName::try_from(GAZE_BUS_NAME) else {
        return false;
    };
    let deadline = Instant::now() + ready_wait;
    loop {
        if let Ok(true) = dbus.name_has_owner(name.clone()).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub(super) async fn check_daemon(
    report: &mut Report,
    username: &str,
    config: Option<&Config>,
    benchmark: bool,
) {
    let service_state = match command_output("systemctl", &["is-active", "gazed"]) {
        Ok((_, state)) => state,
        Err(_) => String::new(),
    };
    let daemon_starting = service_state == "active";
    let ready_wait = if daemon_starting {
        DAEMON_READY_TIMEOUT
    } else {
        Duration::ZERO
    };

    let proxy = match tokio::time::timeout(DAEMON_TIMEOUT, gaze_core::dbus::connect_gaze()).await {
        Ok(Ok(proxy)) => proxy,
        Ok(Err(err)) => {
            report.error(
                "DBus",
                format!("could not reach the system bus: {err}"),
                "Run `systemctl status dbus` and confirm the system bus socket exists.",
            );
            check_cameras(report, config);
            return;
        }
        Err(_) => {
            report.error(
                "DBus",
                "timed out connecting to the system bus",
                "Run `systemctl status dbus` and confirm the system bus socket exists.",
            );
            check_cameras(report, config);
            return;
        }
    };

    let name_wait_started = Instant::now();
    let name_owned = gaze_name_has_owner(&proxy, ready_wait).await;
    // gazed downloads models before it claims the name, so once it is on the bus the
    // remaining budget is all the config call can need. Spend it once, not twice.
    let ready_wait = ready_wait.saturating_sub(name_wait_started.elapsed());

    if name_owned {
        report.pass("DBus", "gazed owns com.gundulabs.Gaze on the system bus");
    } else {
        // Every later call would fail with the same "not activatable" error, so report the
        // cause once instead of repeating it as a camera and an enrollment fault.
        let (message, fix) = if !gaze_core::cpu::supports_inference() {
            (
                "gazed cannot run on this CPU (no AVX2), so it never reaches the system bus"
                    .to_string(),
                gaze_core::cpu::UNSUPPORTED_CPU_FIX.to_string(),
            )
        } else if daemon_starting {
            (
                "gazed is running but has not claimed com.gundulabs.Gaze yet (models may be downloading)"
                    .to_string(),
                "Wait for the first-run model download to finish, then re-run `gaze doctor`."
                    .to_string(),
            )
        } else {
            (
                format!(
                    "gazed is not on the system bus (the service is {})",
                    if service_state.is_empty() {
                        "not reporting a state"
                    } else {
                        service_state.as_str()
                    }
                ),
                "Run `sudo systemctl start gazed`, then `journalctl -u gazed -n 100 --no-pager` if it does not stay up."
                    .to_string(),
            )
        };
        report.error("DBus", message, fix);
        check_cameras(report, config);
        return;
    }

    let mut daemon_config = None;
    match tokio::time::timeout(
        ready_wait + DAEMON_TIMEOUT,
        read_daemon_config(&proxy, ready_wait),
    )
    .await
    {
        Ok(Ok(loaded_config)) => {
            report.pass("Daemon", "gazed responded to a configuration request");
            if config.is_none() {
                for check in config_findings(&loaded_config) {
                    report.checks.push(check);
                }
                check_tpm(report, Some(&loaded_config));
            }
            daemon_config = Some(loaded_config);
        }
        // The name was owned a moment ago, so losing it here means gazed exited mid-check.
        Ok(Err(err)) if dbus_is_not_activatable(&err) => report.error(
            "Daemon",
            "gazed left the system bus while doctor was querying it",
            "Run `journalctl -u gazed -n 100 --no-pager` to see why it exited.",
        ),
        Ok(Err(err)) => report.error(
            "Daemon",
            format!(
                "gazed did not return its configuration: {}",
                dbus_error_message(&err)
            ),
            "Restart gazed and inspect its journal.",
        ),
        Err(_) => report.error(
            "Daemon",
            "gazed timed out while reading its configuration",
            "Restart gazed and inspect its journal.",
        ),
    }
    let config = config.or(daemon_config.as_ref());

    match tokio::time::timeout(DAEMON_TIMEOUT, proxy.is_camera_available()).await {
        Ok(Ok(true)) => report.pass(
            "Camera session",
            "the daemon can access the current PipeWire session",
        ),
        Ok(Ok(false)) => report.error(
            "Camera session",
            "the daemon cannot find a usable PipeWire runtime for this session",
            "Run this command from a local graphical session and verify /run/user/$UID/pipewire-0 exists.",
        ),
        Ok(Err(err)) => report.error(
            "Camera session",
            format!("availability check failed: {}", dbus_error_message(&err)),
            "Inspect the gazed journal for PipeWire or login-session errors.",
        ),
        Err(_) => report.error(
            "Camera session",
            "availability check timed out",
            "Restart gazed and inspect its journal.",
        ),
    }
    check_cameras(report, config);

    match tokio::time::timeout(DAEMON_TIMEOUT, proxy.list_faces(username)).await {
        Ok(Ok(faces)) if faces.is_empty() => report.warning(
            "Enrollment",
            format!("no faces are enrolled for {username}"),
            "Run `gaze add-face default`.",
        ),
        Ok(Ok(faces)) => {
            report.pass(
                "Enrollment",
                format!("{} face profile(s) enrolled for {username}", faces.len()),
            );
            if let Some(config) = config {
                let missing_rgb = !config.cameras.rgb.trim().is_empty()
                    && faces.iter().any(|(_, _, has_rgb, _)| !has_rgb);
                let missing_ir = !config.cameras.ir.trim().is_empty()
                    && faces.iter().any(|(_, _, _, has_ir)| !has_ir);
                if missing_rgb || missing_ir {
                    let spectra = match (missing_rgb, missing_ir) {
                        (true, true) => "RGB and IR",
                        (true, false) => "RGB",
                        (false, true) => "IR",
                        (false, false) => unreachable!(),
                    };
                    report.warning(
                        "Enrollment coverage",
                        format!("one or more profiles have no {spectra} captures"),
                        "Run `gaze refine-face <name>` for profiles missing configured camera spectra.",
                    );
                } else {
                    report.pass(
                        "Enrollment coverage",
                        "all profiles cover the configured camera spectra",
                    );
                }
            }
        }
        Ok(Err(err)) if dbus_is_file_not_found(&err) => report.warning(
            "Enrollment",
            format!("no faces are enrolled for {username}"),
            "Run `gaze add-face default`.",
        ),
        Ok(Err(err)) => report.error(
            "Enrollment",
            format!(
                "could not list faces for {username}: {}",
                dbus_error_message(&err)
            ),
            "Run `gaze list-faces` and inspect the daemon journal.",
        ),
        Err(_) => report.error(
            "Enrollment",
            format!("timed out while checking faces for {username}"),
            "Restart gazed and inspect its journal.",
        ),
    }

    if benchmark {
        check_benchmark(report, &proxy).await;
    }
}

pub(super) async fn check_benchmark(report: &mut Report, proxy: &GazeProxy<'_>) {
    let term = Term::stdout();
    let _ = term.write_line(&format!(
        "{} Benchmarking model inference (this can take a few seconds)...",
        style("i").cyan().bold()
    ));

    let outcome = tokio::time::timeout(BENCHMARK_TIMEOUT, try_benchmark_from_daemon(proxy)).await;
    let _ = term.clear_last_lines(1);

    match outcome {
        Ok(Ok(Some(results))) => {
            for result in results {
                let timings = format!(
                    "{} [{} / {}]: {:.1}ms avg ({:.1} fps), {:.1}ms p95, {:.1}ms min",
                    result.component,
                    result.execution_provider,
                    result.device,
                    result.mean_ms,
                    result.fps,
                    result.p95_ms,
                    result.min_ms
                );
                if result.ran_as_configured() {
                    report.pass("Benchmark", timings);
                } else {
                    report.warning(
                        "Benchmark",
                        format!(
                            "{timings}; configured {}/{} is not in use: {}",
                            result.requested_execution_provider,
                            result.requested_device,
                            if result.fallback_reason.is_empty() {
                                "no reason reported"
                            } else {
                                result.fallback_reason.as_str()
                            }
                        ),
                        "Check the gazed journal for the OpenVINO setup error, or set [inference] back to cpu/cpu.",
                    );
                }
            }
        }
        Ok(Ok(None)) => report.warning(
            "Benchmark",
            "the running daemon reports a benchmark layout this build cannot read",
            "Restart it with `systemctl restart gazed`.",
        ),
        Ok(Err(err)) => report.warning(
            "Benchmark",
            format!("gazed could not run the benchmark: {err}"),
            "Restart gazed and inspect its journal.",
        ),
        Err(_) => report.warning(
            "Benchmark",
            "benchmark timed out",
            "Restart gazed and inspect its journal.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(gids: &[u32], dac_override: bool) -> DaemonCredentials {
        DaemonCredentials {
            uid: 0,
            gids: gids.to_vec(),
            dac_override,
        }
    }

    #[test]
    fn root_owned_tpm_node_is_openable_without_dac_override() {
        let creds = credentials(&[0], false);
        assert!(node_openable(0, 972, 0o660, &creds));
        assert!(!node_openable(0, 972, 0o060, &creds));
    }

    #[test]
    fn tss_owned_tpm_node_needs_the_group_or_the_capability() {
        let tss_uid = 972;
        let tss_gid = 972;

        let bare_root = credentials(&[0], false);
        assert!(
            !node_openable(tss_uid, tss_gid, 0o660, &bare_root),
            "the reporter's Ubuntu node must read as unopenable"
        );

        assert!(node_openable(
            tss_uid,
            tss_gid,
            0o660,
            &credentials(&[0, tss_gid], false)
        ));
        assert!(node_openable(
            tss_uid,
            tss_gid,
            0o660,
            &credentials(&[0], true)
        ));
        assert!(node_openable(tss_uid, tss_gid, 0o666, &bare_root));
    }

    #[test]
    fn group_access_does_not_rescue_an_owner_class_mismatch() {
        let creds = credentials(&[0, 972], false);
        assert!(!node_openable(0, 972, 0o060, &creds));
    }

    #[test]
    fn daemon_credentials_come_from_the_effective_unit() {
        let parsed = parse_daemon_credentials(
            "LoadState=loaded\nCapabilityBoundingSet=cap_dac_read_search cap_dac_override\nUser=\nSupplementaryGroups=video",
        )
        .expect("a loaded root unit must be understood");
        assert_eq!(parsed.uid, 0);
        assert!(parsed.dac_override);
        assert!(parsed.gids.contains(&0));

        let capless = parse_daemon_credentials(
            "LoadState=loaded\nCapabilityBoundingSet=cap_dac_read_search\nUser=\nSupplementaryGroups=video",
        )
        .expect("a loaded root unit must be understood");
        assert!(!capless.dac_override);

        assert_eq!(
            parse_daemon_credentials("LoadState=not-found\nUser=\nCapabilityBoundingSet="),
            None,
            "an uninstalled unit must not be judged"
        );
        assert_eq!(
            parse_daemon_credentials(
                "LoadState=loaded\nUser=gaze\nCapabilityBoundingSet=cap_dac_read_search"
            ),
            None,
            "a custom User= must not be judged"
        );
    }
}
