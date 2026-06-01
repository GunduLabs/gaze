use crate::config::Config;
use serde::{Deserialize, Serialize};
use zbus::proxy;
use zbus::zvariant::Type;

use strum_macros::{AsRefStr, Display, EnumString, VariantNames};

#[derive(
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    Type,
    PartialEq,
    Eq,
    Display,
    EnumString,
    AsRefStr,
    VariantNames,
)]
#[zvariant(signature = "s")]
#[serde(rename_all = "kebab-case")]
pub enum CaptureStatus {
    #[strum(serialize = "Please look at the camera...")]
    NoFace,
    #[strum(serialize = "Need more light...")]
    TooDark,
    #[strum(serialize = "Face is clipped. Please move back...")]
    Clipped,
    #[strum(serialize = "Please center your face...")]
    NotCentered,
    #[strum(serialize = "Please come closer...")]
    TooFar,
    #[strum(serialize = "Please back up...")]
    TooClose,
    #[strum(serialize = "Hold still...")]
    Ready,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    Type,
    PartialEq,
    Eq,
    Display,
    EnumString,
    AsRefStr,
    VariantNames,
)]
#[zvariant(signature = "s")]
#[serde(rename_all = "kebab-case")]
pub enum EnrollPrompt {
    #[strum(serialize = "Face the camera")]
    LookStraight,
    #[strum(serialize = "Tilt your face slightly up")]
    LookUp,
    #[strum(serialize = "Tilt your face slightly down")]
    LookDown,
    #[strum(serialize = "Turn your face slightly left")]
    LookLeft,
    #[strum(serialize = "Turn your face slightly right")]
    LookRight,
    #[strum(serialize = "Database error during enrollment")]
    DbFailed,
    #[strum(serialize = "Enrollment cancelled")]
    Cancelled,
    #[strum(serialize = "Captured")]
    Captured,
    #[strum(serialize = "Completed")]
    Completed,
}

#[derive(
    Clone,
    Copy,
    Debug,
    Serialize,
    Deserialize,
    Type,
    PartialEq,
    Eq,
    Display,
    EnumString,
    AsRefStr,
    VariantNames,
)]
#[zvariant(signature = "s")]
#[serde(rename_all = "kebab-case")]
pub enum VerifyResult {
    VerifyMatch,
    VerifyNoMatch,
}

use std::collections::HashMap;
use zvariant::OwnedValue;

pub fn dbus_error_message(err: &zbus::Error) -> String {
    let text = err.to_string();
    if let Some((_, inner)) = text.split_once(':') {
        return inner.trim().to_string();
    }
    text
}

pub fn dbus_is_file_not_found(err: &zbus::Error) -> bool {
    err.to_string().contains("FileNotFound")
}

pub async fn load_config_from_daemon(proxy: &GazeProxy<'_>) -> anyhow::Result<Config> {
    let map = proxy.get_config().await?;
    Config::from_map(map)
}

pub async fn apply_config_to_daemon(proxy: &GazeProxy<'_>, config: &Config) -> anyhow::Result<()> {
    proxy.set_config(config.to_map()).await?;
    Ok(())
}

#[proxy(
    interface = "com.gundulabs.Gaze",
    default_service = "com.gundulabs.Gaze",
    default_path = "/com/gundulabs/Gaze"
)]
pub trait Gaze {
    async fn claim(&self, username: &str) -> zbus::Result<()>;
    async fn release(&self) -> zbus::Result<()>;

    async fn verify_start(&self, face_name: &str) -> zbus::Result<()>;
    async fn verify_stop(&self) -> zbus::Result<()>;

    async fn enroll_start(&self, face_name: &str) -> zbus::Result<()>;
    async fn enroll_stop(&self) -> zbus::Result<()>;

    async fn list_faces(&self, username: &str) -> zbus::Result<Vec<(String, u32)>>;
    async fn delete_face(&self, username: &str, face_name: &str) -> zbus::Result<bool>;
    async fn rename_face(
        &self,
        username: &str,
        old_face_name: &str,
        new_face_name: &str,
    ) -> zbus::Result<bool>;
    async fn delete_faces(&self, username: &str) -> zbus::Result<bool>;

    #[zbus(allow_interactive_auth)]
    async fn get_config(&self) -> zbus::Result<HashMap<String, HashMap<String, OwnedValue>>>;
    #[zbus(allow_interactive_auth)]
    async fn set_config(
        &self,
        config: HashMap<String, HashMap<String, OwnedValue>>,
    ) -> zbus::Result<bool>;

    async fn get_gdm_face_auth(&self) -> zbus::Result<bool>;
    #[zbus(allow_interactive_auth)]
    async fn set_gdm_face_auth(&self, enabled: bool) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn face_status(&self, status: CaptureStatus) -> zbus::Result<()>;

    #[zbus(signal)]
    fn verify_status(
        &self,
        result: VerifyResult,
        faces: Vec<(String, f64, f64, bool, u32)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    fn enroll_status(
        &self,
        face_name: &str,
        progress: u32,
        max: u32,
        is_done: bool,
        msg: EnrollPrompt,
        time_remaining: f64,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_display_strings_are_user_facing_messages() {
        assert_eq!(
            CaptureStatus::NoFace.to_string(),
            "Please look at the camera..."
        );
        assert_eq!(CaptureStatus::TooDark.to_string(), "Need more light...");
        assert_eq!(CaptureStatus::Ready.to_string(), "Hold still...");
        assert_eq!(
            EnrollPrompt::LookLeft.to_string(),
            "Turn your face slightly left"
        );
        assert_eq!(VerifyResult::VerifyNoMatch.as_ref(), "VerifyNoMatch");
    }

    #[test]
    fn serde_plain_uses_kebab_case_wire_values() {
        assert_eq!(
            serde_plain::to_string(&CaptureStatus::TooClose).unwrap(),
            "too-close"
        );
        assert_eq!(
            serde_plain::to_string(&CaptureStatus::TooDark).unwrap(),
            "too-dark"
        );
        assert_eq!(
            serde_plain::to_string(&EnrollPrompt::LookStraight).unwrap(),
            "look-straight"
        );
        assert_eq!(
            serde_plain::to_string(&VerifyResult::VerifyMatch).unwrap(),
            "verify-match"
        );

        assert_eq!(
            serde_plain::from_str::<CaptureStatus>("not-centered").unwrap(),
            CaptureStatus::NotCentered
        );
        assert_eq!(
            serde_plain::from_str::<EnrollPrompt>("db-failed").unwrap(),
            EnrollPrompt::DbFailed
        );
        assert_eq!(
            serde_plain::from_str::<VerifyResult>("verify-no-match").unwrap(),
            VerifyResult::VerifyNoMatch
        );
    }

    #[test]
    fn dbus_error_helpers_parse_display_text() {
        let err = zbus::Error::Failure("org.example.Error: useful detail".to_string());
        assert_eq!(dbus_error_message(&err), "useful detail");
        assert!(!dbus_is_file_not_found(&err));

        let err = zbus::Error::Failure("FileNotFound: missing face".to_string());
        assert_eq!(dbus_error_message(&err), "missing face");
        assert!(dbus_is_file_not_found(&err));

        let err = zbus::Error::Failure("plain failure".to_string());
        assert_eq!(dbus_error_message(&err), "plain failure");
    }
}
