// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use console::{Term, style};
use dialoguer::{Confirm, theme::ColorfulTheme};
use gaze_core::dbus::{GazeProxy, dbus_is_file_not_found, try_load_config_from_daemon};

use crate::tui::Tone;
use crate::{describe_dbus_error, exit_with_error, interactive_terminal, json_string, run_busy};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpectrumBadge {
    /// The profile holds captures for this spectrum.
    Enrolled,
    /// A camera is configured for this spectrum, but the profile has no captures yet.
    Missing,
    /// No camera for this spectrum, so there is nothing to enroll.
    Unused,
}

/// A spectrum without a configured camera is not missing from the profile.
/// Describe it as unconfigured rather than incomplete.
fn spectrum_badge_state(enrolled: bool, configured: bool) -> SpectrumBadge {
    match (enrolled, configured) {
        (true, _) => SpectrumBadge::Enrolled,
        (false, true) => SpectrumBadge::Missing,
        (false, false) => SpectrumBadge::Unused,
    }
}

fn spectrum_badge(label: &str, enrolled: bool, configured: bool) -> String {
    let badge = format!("[{label}]");
    match spectrum_badge_state(enrolled, configured) {
        SpectrumBadge::Enrolled => style(badge).green().bold().to_string(),
        SpectrumBadge::Missing => style(badge).yellow().bold().to_string(),
        SpectrumBadge::Unused => style(badge).dim().to_string(),
    }
}

fn write_no_faces(term: &Term, user: &str) -> anyhow::Result<()> {
    term.write_line(&format!(
        "{} No faces found for {}",
        style("i").cyan().bold(),
        style(user).bold()
    ))?;
    Ok(())
}

async fn print_faces_json(proxy: &GazeProxy<'_>, user: &str) -> anyhow::Result<()> {
    let faces = match proxy.list_faces(user).await {
        Ok(faces) => faces,
        Err(err) if dbus_is_file_not_found(&err) => Vec::new(),
        Err(err) => {
            exit_with_error(&describe_dbus_error(proxy, "Failed to fetch faces", &err).await)
        }
    };
    let entries: Vec<String> = faces
        .iter()
        .map(|(face, count, has_rgb, has_ir)| {
            format!(
                "{{\"name\":{},\"captures\":{count},\"rgb\":{has_rgb},\"ir\":{has_ir}}}",
                json_string(face)
            )
        })
        .collect();
    println!(
        "{{\"user\":{},\"faces\":[{}]}}",
        json_string(user),
        entries.join(",")
    );
    Ok(())
}

pub async fn handle_list_faces(
    proxy: &GazeProxy<'_>,
    user: &str,
    json: bool,
) -> anyhow::Result<()> {
    if json {
        return print_faces_json(proxy, user).await;
    }
    let term = Term::stdout();
    let cameras = try_load_config_from_daemon(proxy)
        .await
        .ok()
        .flatten()
        .map(|config| {
            (
                !config.cameras.rgb.trim().is_empty(),
                !config.cameras.ir.trim().is_empty(),
            )
        });
    let (rgb_configured, ir_configured) = cameras.unwrap_or((true, true));
    let result = run_busy(
        "Face database",
        format!("Fetching faces for {user}..."),
        Tone::Info,
        proxy.list_faces(user),
    )
    .await?;

    match result {
        Ok(faces) => {
            if faces.is_empty() {
                write_no_faces(&term, user)?;
            } else {
                term.write_line(&format!(
                    "\n{} face{} for {}:\n",
                    style(faces.len()).green().bold(),
                    if faces.len() == 1 { "" } else { "s" },
                    style(user).bold()
                ))?;
                for (face, count, has_rgb, has_ir) in faces {
                    let rgb_badge = spectrum_badge("RGB", has_rgb, rgb_configured);
                    let ir_badge = spectrum_badge("IR", has_ir, ir_configured);
                    term.write_line(&format!(
                        "  {} {} {} {} ({} capture{})",
                        style("•").cyan(),
                        style(face).bold(),
                        rgb_badge,
                        ir_badge,
                        count,
                        if count == 1 { "" } else { "s" }
                    ))?;
                }
                term.write_line("")?;
            }
        }
        Err(e) => {
            if dbus_is_file_not_found(&e) {
                write_no_faces(&term, user)?;
            } else {
                exit_with_error(&describe_dbus_error(proxy, "Failed to fetch faces", &e).await);
            }
        }
    }
    Ok(())
}

fn exit_face_not_found(face: &str, user: &str) -> ! {
    exit_with_error(&format!(
        "Face '{face}' not found for '{user}'. Run `gaze list-faces` to see the enrolled faces."
    ))
}

pub async fn handle_remove_face(
    proxy: &GazeProxy<'_>,
    user: &str,
    face: &str,
) -> anyhow::Result<()> {
    let term = Term::stdout();
    let result = run_busy(
        "Remove face",
        format!("Deleting face {face}..."),
        Tone::Warn,
        proxy.delete_face(user, face),
    )
    .await?;

    match result {
        Ok(true) => {
            term.write_line(&format!(
                "{} Face '{}' removed for '{}'",
                style("✓").green().bold(),
                face,
                user
            ))?;
        }
        Ok(false) => exit_face_not_found(face, user),
        Err(err) if dbus_is_file_not_found(&err) => exit_face_not_found(face, user),
        Err(err) => {
            exit_with_error(&describe_dbus_error(proxy, "Failed to remove face", &err).await)
        }
    }
    Ok(())
}

pub async fn handle_rename_face(
    proxy: &GazeProxy<'_>,
    user: &str,
    from: &str,
    to: &str,
) -> anyhow::Result<()> {
    let term = Term::stdout();
    let result = run_busy(
        "Rename face",
        format!("Renaming face {from} -> {to}..."),
        Tone::Info,
        proxy.rename_face(user, from, to),
    )
    .await?;

    match result {
        Ok(true) => {
            term.write_line(&format!(
                "{} Face '{}' renamed to '{}' for '{}'",
                style("✓").green().bold(),
                from,
                to,
                user
            ))?;
        }
        Ok(false) => exit_face_not_found(from, user),
        Err(err) if dbus_is_file_not_found(&err) => exit_face_not_found(from, user),
        Err(err) => {
            exit_with_error(&describe_dbus_error(proxy, "Failed to rename face", &err).await)
        }
    }
    Ok(())
}

pub async fn handle_duress(proxy: &GazeProxy<'_>, user: &str, clear: bool) -> anyhow::Result<()> {
    let term = Term::stdout();
    if clear {
        let cleared = match proxy.clear_duress(user).await {
            Ok(cleared) => cleared,
            Err(err) => exit_with_error(
                &describe_dbus_error(proxy, "Failed to clear the duress lockout", &err).await,
            ),
        };
        if cleared {
            term.write_line(&format!(
                "Face authentication re-enabled for {}.",
                style(user).bold()
            ))?;
        } else {
            term.write_line(&format!(
                "Face authentication was not locked for {}.",
                style(user).bold()
            ))?;
        }
        return Ok(());
    }

    let locked = match proxy.duress_locked(user).await {
        Ok(locked) => locked,
        Err(err) => exit_with_error(
            &describe_dbus_error(proxy, "Failed to read the duress lockout", &err).await,
        ),
    };
    if locked {
        term.write_line(&format!(
            "Face authentication for {} is {} after a duress signal. Log in with your password, \
             or run `gaze duress --clear`.",
            style(user).bold(),
            style("locked").red().bold()
        ))?;
    } else {
        term.write_line(&format!(
            "Face authentication for {} is {}.",
            style(user).bold(),
            style("not locked").green()
        ))?;
    }
    Ok(())
}

pub async fn handle_clear_user(proxy: &GazeProxy<'_>, user: &str, yes: bool) -> anyhow::Result<()> {
    let term = Term::stdout();
    if !yes {
        if !interactive_terminal() {
            anyhow::bail!(
                "gaze clear-user deletes every face and stored keyring credential for '{user}'; \
                 pass --yes to confirm"
            );
        }
        let proceed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "Delete every enrolled face and stored keyring or KWallet credential for '{user}'?"
            ))
            .default(false)
            .interact()?;
        if !proceed {
            term.write_line("Cancelled; nothing was deleted.")?;
            return Ok(());
        }
    }
    let result = run_busy(
        "Clear user",
        format!("Deleting all data for {user}..."),
        Tone::Warn,
        proxy.delete_faces(user),
    )
    .await?;

    match result {
        Ok(_) => {
            gaze_security::keyring::forget(user)
                .and_then(|()| {
                    gaze_security::keyring::forget_for(
                        gaze_security::keyring::Backend::KWallet,
                        user,
                    )
                })
                .map_err(|err| {
                    anyhow::anyhow!(
                        "Face data cleared, but could not remove a stored keyring credential \
                     for '{user}': {err}"
                    )
                })?;
            term.write_line(&format!(
                "{} All data cleared for '{}'",
                style("✓").green().bold(),
                user
            ))?;
        }
        Err(err) => {
            exit_with_error(&describe_dbus_error(proxy, "Failed to clear user", &err).await)
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unconfigured_spectrum_reads_as_unlit_not_as_a_failure() {
        assert_eq!(spectrum_badge_state(true, true), SpectrumBadge::Enrolled);
        assert_eq!(
            spectrum_badge_state(true, false),
            SpectrumBadge::Enrolled,
            "captures already taken stay green even after the camera is unset"
        );
        assert_eq!(
            spectrum_badge_state(false, true),
            SpectrumBadge::Missing,
            "a configured camera the profile never captured is a real gap"
        );
        assert_eq!(
            spectrum_badge_state(false, false),
            SpectrumBadge::Unused,
            "an RGB-only machine has nothing to enroll for IR, so IR is not a failure"
        );
        assert!(spectrum_badge("RGB", true, true).contains("[RGB]"));
    }
}
