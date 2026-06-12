mod camera_view;
mod capture_dialog;
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
            .success { color: #2ec27e; }
            .error { color: #e01b24; }
            .badge-success {
                background-color: #2ec27e;
                color: white;
                border-radius: 6px;
                padding: 1px 6px;
                font-weight: bold;
                font-size: 0.75rem;
            }
            .badge-error {
                background-color: #e01b24;
                color: white;
                border-radius: 6px;
                padding: 1px 6px;
                font-weight: bold;
                font-size: 0.75rem;
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
