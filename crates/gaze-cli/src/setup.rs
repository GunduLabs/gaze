// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use console::Term;
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use gaze_core::{config::Config, dbus::GazeProxy};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// Read a frame without enrolling or changing settings. Release the camera before continuing.
pub async fn check_cameras(config: &Config) -> anyhow::Result<()> {
    let cameras = config.cameras.clone();
    anyhow::ensure!(
        !cameras.rgb.is_empty() || !cameras.ir.is_empty(),
        "No camera selected. Choose a color or infrared camera in Settings"
    );
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        use gaze_vision::camera::Camera;
        if !cameras.rgb.is_empty() {
            let mut camera = Camera::open_rgb(&cameras.rgb)?;
            anyhow::ensure!(
                camera.next_interruptible(&worker_stop).is_some(),
                "No frame received from the color camera"
            );
        }
        if !cameras.ir.is_empty() && !worker_stop.load(Ordering::Relaxed) {
            let mut camera = Camera::open_ir(&cameras.ir, cameras.ir_frame_size())?;
            anyhow::ensure!(
                camera.next_interruptible(&worker_stop).is_some(),
                "No frame received from the infrared camera"
            );
        }
        Ok(())
    });
    let mut worker = worker;
    let result = tokio::time::timeout(Duration::from_secs(10), &mut worker).await;
    stop.store(true, Ordering::Relaxed);
    match result {
        Ok(result) => result??,
        Err(_) => anyhow::bail!(
            "Camera check timed out. Close other camera apps and check the privacy shutter"
        ),
    }
    Ok(())
}

pub async fn run(proxy: &GazeProxy<'_>) -> anyhow::Result<()> {
    let theme = ColorfulTheme::default();
    let user = crate::get_current_user();
    println!(
        "Gaze setup\n1. Check camera  →  2. Enroll  →  3. Test  →  4. Check desktop integration"
    );
    loop {
        let (mut config, support) =
            gaze_core::dbus::load_config_with_keyring_from_daemon(proxy).await?;
        let desktop = format!(
            "{} {}",
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
            std::env::var("DESKTOP_SESSION").unwrap_or_default()
        )
        .to_ascii_lowercase();
        if config.auth.start_delay_ms == 0
            && (desktop.contains("kde")
                || desktop.contains("plasma")
                || desktop.contains("hyprland"))
            && Confirm::with_theme(&theme)
                .with_prompt("Wait 3 seconds after locking to give you time to step away")
                .default(true)
                .interact()?
        {
            proxy.authorize_settings().await?;
            config.auth.start_delay_ms = 3000;
            config.auth.start_delay_scope = "screen_lock".into();
            gaze_core::dbus::apply_config_with_keyring_to_daemon(proxy, &config).await?;
            println!("✓ Lock-screen delay saved; sudo and login prompts still start immediately.");
        }
        println!(
            "Color camera: {} | Infrared: {}",
            config.cameras.rgb,
            if config.cameras.ir.is_empty() {
                "None"
            } else {
                &config.cameras.ir
            }
        );
        let result = crate::run_busy(
            "Checking camera",
            "Checking that the selected cameras produce frames…".into(),
            crate::tui::Tone::Info,
            false,
            check_cameras(&config),
        )
        .await?;
        if let Err(error) = &result {
            eprintln!(
                "Camera check: {error}. Infrared access may require the daemon; enrollment will check it again."
            );
        }
        match Select::with_theme(&theme)
            .with_prompt("Camera setup")
            .items([
                "Continue to enrollment",
                "Choose cameras in settings",
                "Exit setup",
            ])
            .default(if result.is_ok() { 0 } else { 1 })
            .interact_opt()?
        {
            Some(0) => break,
            Some(1) => {
                proxy.authorize_settings().await?;
                crate::configure::run_config_wizard(&Term::stdout(), proxy, config, support).await?
            }
            _ => return Ok(()),
        }
    }
    let faces = proxy.list_faces(&user).await.unwrap_or_default();
    if !proxy.has_enrolled_faces(&user).await?
        || Confirm::with_theme(&theme)
            .with_prompt("Enroll another profile")
            .default(false)
            .interact()?
    {
        let suggested = if faces.is_empty() {
            "default".to_string()
        } else {
            (2..)
                .map(|n| format!("face-{n}"))
                .find(|name| !faces.iter().any(|face| &face.0 == name))
                .unwrap()
        };
        let name: String = Input::with_theme(&theme)
            .with_prompt("Profile name")
            .default(suggested)
            .validate_with(|name: &String| {
                if name.trim().is_empty() || faces.iter().any(|face| face.0 == name.trim()) {
                    Err("Use a new, non-empty profile name")
                } else {
                    Ok(())
                }
            })
            .interact_text()?;
        crate::handle_enroll(proxy, &user, name.trim(), 0, false).await?;
    }
    crate::handle_test(proxy, &user, false, false).await?;
    println!("Checking desktop integration…");
    let healthy = crate::doctor::run(&user, false, crate::doctor::Output::Human).await?;
    if healthy {
        println!(
            "Recognition works. Review any desktop instructions above before locking your screen; login and keyring unlock are separate optional settings."
        );
    } else {
        anyhow::bail!(
            "Recognition works, but setup still needs attention. Resolve the checks above and run gaze setup again"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn camera_check_rejects_an_empty_configuration() {
        let mut config = Config::default();
        config.cameras.rgb = "".into();
        config.cameras.ir.clear();
        let error = check_cameras(&config).await.unwrap_err();
        assert!(error.to_string().contains("No camera selected"));
    }
}
