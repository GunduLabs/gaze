// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use zbus::{OwnedVariant, Type, Variant};

pub const ALLOW_USER_INTERACTION: u32 = 1;
pub const NO_INTERACTION: u32 = 0;

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct Subject {
    kind: String,
    details: HashMap<String, OwnedVariant>,
}

impl Subject {
    pub fn system_bus_name(name: &str) -> zbus::Result<Self> {
        Ok(Self {
            kind: "system-bus-name".to_string(),
            details: HashMap::from([(
                "name".to_string(),
                OwnedVariant::try_from(Variant::from(name.to_string()))?,
            )]),
        })
    }

    pub fn for_message_header(header: &zbus::message::Header<'_>) -> zbus::Result<Self> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::Error::Failure("message has no sender".to_string()))?;
        Self::system_bus_name(sender.as_str())
    }

    pub fn unix_process(pid: u32) -> anyhow::Result<Self> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let fields = stat
            .rsplit_once(')')
            .map(|(_, rest)| rest)
            .ok_or_else(|| anyhow::anyhow!("unreadable /proc/{pid}/stat"))?;
        let start_time: u64 = fields
            .split_whitespace()
            .nth(19)
            .ok_or_else(|| anyhow::anyhow!("/proc/{pid}/stat has no start time"))?
            .parse()?;
        let uid = std::fs::metadata(format!("/proc/{pid}"))?.uid() as i32;
        Ok(Self {
            kind: "unix-process".to_string(),
            details: HashMap::from([
                (
                    "pid".to_string(),
                    OwnedVariant::try_from(Variant::from(pid))?,
                ),
                (
                    "start-time".to_string(),
                    OwnedVariant::try_from(Variant::from(start_time))?,
                ),
                (
                    "uid".to_string(),
                    OwnedVariant::try_from(Variant::from(uid))?,
                ),
            ]),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Type)]
pub struct AuthorizationResult {
    pub is_authorized: bool,
    pub is_challenge: bool,
    pub details: HashMap<String, String>,
}

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
pub trait Authority {
    async fn check_authorization(
        &self,
        subject: &Subject,
        action_id: &str,
        details: &HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<AuthorizationResult>;

    #[zbus(signal)]
    fn changed(&self) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_and_results_match_the_polkit_signatures() {
        assert_eq!(Subject::SIGNATURE.to_string(), "(sa{sv})");
        assert_eq!(AuthorizationResult::SIGNATURE.to_string(), "(bba{ss})");
    }

    #[test]
    fn a_process_subject_names_this_process() {
        let subject = Subject::unix_process(std::process::id()).unwrap();
        assert_eq!(subject.kind, "unix-process");
        for key in ["pid", "start-time", "uid"] {
            assert!(subject.details.contains_key(key), "missing {key}");
        }
    }
}
