// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

mod camera;
mod dialog;
mod window;

use gtk4::prelude::*;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());

    let app = libadwaita::Application::builder()
        .application_id("com.gundulabs.Gaze")
        .build();

    app.connect_activate(move |app| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(
            r#"
            .badge-success,
            .badge-warning,
            .badge-muted {
                border-radius: 999px;
                padding: 2px 8px;
                font-weight: 600;
                font-size: 0.75rem;
            }
            .badge-success {
                background-color: alpha(@success_bg_color, 0.18);
                color: @success_color;
            }
            .badge-warning {
                background-color: alpha(@warning_bg_color, 0.2);
                color: @warning_color;
            }
            .badge-muted {
                background-color: alpha(currentColor, 0.08);
                color: alpha(currentColor, 0.55);
            }
            .step-marker {
                min-width: 26px;
                min-height: 26px;
                border-radius: 999px;
                background-color: alpha(currentColor, 0.1);
                font-weight: bold;
                font-size: 0.85rem;
            }
            .step-marker.done {
                background-color: @success_bg_color;
                color: @success_fg_color;
            }
            .camera-frame {
                border-radius: 12px;
            }
            .report-text {
                padding: 12px;
                border-radius: 8px;
            }
            "#,
        );
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(),
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        window::build_window(app, &username);
    });

    app.run();
}
