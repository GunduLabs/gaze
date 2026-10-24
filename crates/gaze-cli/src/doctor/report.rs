// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

const JOURNAL_LINES: &str = "200";

pub(super) fn write(report: &Report, username: &str) -> anyhow::Result<PathBuf> {
    let mut out = String::new();
    section(
        &mut out,
        "gaze doctor",
        &console::strip_ansi_codes(&report.render()),
    );
    section(&mut out, "gaze version", env!("CARGO_PKG_VERSION"));
    section(
        &mut out,
        "/etc/os-release",
        &fs::read_to_string("/etc/os-release").unwrap_or_else(|e| format!("unreadable: {e}")),
    );
    section(&mut out, "kernel", &run("uname", &["-srm"]));
    section(&mut out, "desktop session", &desktop_session());
    section(&mut out, "installed Gaze packages", &installed_packages());
    section(&mut out, "PAM lines that mention Gaze", &pam_lines());
    section(
        &mut out,
        "gazed service",
        &run("systemctl", &["status", "gazed", "--no-pager", "--lines=0"]),
    );
    section(
        &mut out,
        "gazed journal",
        &run(
            "journalctl",
            &[
                "-u",
                "gazed",
                "-n",
                JOURNAL_LINES,
                "--no-pager",
                "-o",
                "short-iso",
            ],
        ),
    );
    section(
        &mut out,
        CONFIG_PATH,
        &fs::read_to_string(CONFIG_PATH).unwrap_or_else(|e| format!("unreadable: {e}")),
    );

    let text = redact(&out, username, &hostname());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let name = format!("gaze-report-{stamp}.txt");
    match create_private(Path::new(&name), &text) {
        Ok(()) => Ok(PathBuf::from(name)),
        Err(_) => {
            let fallback = std::env::temp_dir().join(&name);
            create_private(&fallback, &text)?;
            Ok(fallback)
        }
    }
}

fn create_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

fn section(out: &mut String, title: &str, body: &str) {
    out.push_str(&format!("===== {title} =====\n"));
    out.push_str(body.trim_end());
    out.push_str("\n\n");
}

fn run(program: &str, args: &[&str]) -> String {
    match command_output(program, args) {
        Ok((_, text)) if !text.is_empty() => text,
        Ok(_) => "(no output)".to_string(),
        Err(e) => format!("{program} unavailable: {e}"),
    }
}

fn desktop_session() -> String {
    [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_SESSION_TYPE",
        "DESKTOP_SESSION",
    ]
    .iter()
    .map(|key| format!("{key}={}", std::env::var(key).unwrap_or_default()))
    .collect::<Vec<_>>()
    .join("\n")
}

fn installed_packages() -> String {
    let queries: [(&str, &[&str]); 3] = [
        ("dpkg-query", &["-W", "-f=${Package} ${Version}\n", "gaze*"]),
        ("rpm", &["-qa", "gaze*"]),
        ("pacman", &["-Qs", "^gaze"]),
    ];
    let found: Vec<String> = queries
        .iter()
        .filter_map(|(program, args)| match command_output(program, args) {
            Ok((true, text)) if !text.is_empty() => Some(text),
            _ => None,
        })
        .collect();
    if found.is_empty() {
        "no Gaze packages found through dpkg, rpm or pacman".to_string()
    } else {
        found.join("\n")
    }
}

fn pam_lines() -> String {
    let mut lines = Vec::new();
    for dir in ["/etc/pam.d", VENDOR_PAM_DIR] {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            let Ok(contents) = fs::read_to_string(&path) else {
                continue;
            };
            for line in contents.lines() {
                let trimmed = line.trim();
                if !trimmed.starts_with('#') && trimmed.contains("gaze") {
                    lines.push(format!("{}: {trimmed}", path.display()));
                }
            }
        }
    }
    if lines.is_empty() {
        "no PAM service references Gaze".to_string()
    } else {
        lines.join("\n")
    }
}

fn hostname() -> String {
    fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .or_else(|| command_output("uname", &["-n"]).ok().map(|(_, name)| name))
        .unwrap_or_default()
}

fn redact(text: &str, username: &str, hostname: &str) -> String {
    let mut text = text.to_string();
    for (secret, placeholder) in [(hostname, "<host>"), (username, "<user>")] {
        if secret.len() >= 3 && secret != "root" {
            text = text.replace(secret, placeholder);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_hides_the_user_and_host_but_keeps_short_or_generic_names() {
        let text = "alice@laptop-7: /home/alice/.config ok; root and al stay";
        let redacted = redact(text, "alice", "laptop-7");
        assert_eq!(
            redacted,
            "<user>@<host>: /home/<user>/.config ok; root and al stay"
        );
        assert_eq!(redact("root login", "root", ""), "root login");
        assert_eq!(redact("al ok", "al", ""), "al ok");
    }
}
