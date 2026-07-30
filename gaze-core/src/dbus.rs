// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(unreachable_patterns)]
use crate::config::Config;
use serde::{Deserialize, Serialize};
use zbus::proxy;
use zbus::zvariant::{OwnedValue, Type, Value};

use strum_macros::{AsRefStr, Display, EnumString, VariantNames};

#[allow(unreachable_patterns)]
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
    #[strum(serialize = "Camera is not in use...")]
    Unused,
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
    #[strum(serialize = "Hold still...")]
    Usable,
}

impl CaptureStatus {
    pub fn priority(self) -> u8 {
        match self {
            Self::Usable => 5,
            Self::Ready => 4,
            Self::NotCentered | Self::TooFar | Self::TooClose | Self::Clipped => 3,
            Self::TooDark => 2,
            Self::NoFace => 1,
            Self::Unused => 0,
        }
    }
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
    #[strum(serialize = "Camera error during enrollment")]
    CameraFailed,
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

#[derive(Clone, Debug, Serialize, Deserialize, Value, OwnedValue, Type)]
pub struct BenchmarkResult {
    pub component: String,
    pub execution_provider: String,
    pub device: String,
    pub requested_execution_provider: String,
    pub requested_device: String,
    pub fallback_reason: String,
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub min_ms: f64,
    pub fps: f64,
}

impl BenchmarkResult {
    pub fn ran_as_configured(&self) -> bool {
        self.fallback_reason.is_empty()
            && self.execution_provider == self.requested_execution_provider
            && self.device == self.requested_device
    }
}

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

pub fn dbus_is_not_activatable(err: &zbus::Error) -> bool {
    let s = err.to_string();
    s.contains("not activatable") || s.contains("ServiceUnknown")
}

pub async fn connect_gaze() -> zbus::Result<GazeProxy<'static>> {
    let connection = zbus::Connection::system().await?;
    GazeProxy::new(&connection).await
}

pub async fn try_benchmark_from_daemon(
    proxy: &GazeProxy<'_>,
) -> anyhow::Result<Option<Vec<BenchmarkResult>>> {
    let raw: OwnedValue = proxy
        .inner()
        .call("Benchmark", &())
        .await
        .map_err(|e| anyhow::anyhow!("{}", dbus_error_message(&e)))?;
    benchmark_from_reply(raw)
}

pub fn benchmark_from_reply(raw: OwnedValue) -> anyhow::Result<Option<Vec<BenchmarkResult>>> {
    let expected = <Vec<BenchmarkResult> as Type>::SIGNATURE;
    let actual = raw.value_signature();
    if actual != expected {
        tracing::warn!(
            %actual,
            %expected,
            "daemon benchmark layout does not match this build; restart gazed"
        );
        return Ok(None);
    }

    Vec::<BenchmarkResult>::try_from(raw)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("Failed to decode benchmark results: {}", e))
}

pub async fn try_load_config_from_daemon(proxy: &GazeProxy<'_>) -> anyhow::Result<Option<Config>> {
    let raw: OwnedValue = proxy
        .inner()
        .get_property("Config")
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read config property: {}", e))?;
    config_from_property(raw)
}

pub fn config_from_property(raw: OwnedValue) -> anyhow::Result<Option<Config>> {
    let expected = <Config as Type>::SIGNATURE;
    let actual = raw.value_signature();
    if actual != expected {
        tracing::warn!(
            %actual,
            %expected,
            "daemon config layout does not match this build; restart gazed"
        );
        return Ok(None);
    }

    Config::try_from(raw)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("Failed to decode config property: {}", e))
}

pub async fn load_config_from_daemon(proxy: &GazeProxy<'_>) -> anyhow::Result<Config> {
    try_load_config_from_daemon(proxy).await?.ok_or_else(|| {
        anyhow::anyhow!(
            "The running daemon predates this build and sends a config layout it \
             cannot read. Restart it with `systemctl restart gazed`."
        )
    })
}

pub async fn apply_config_to_daemon(proxy: &GazeProxy<'_>, config: &Config) -> anyhow::Result<()> {
    proxy
        .set_config(config.clone())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to set config property: {}", e))
}

pub async fn get_active_session_uid() -> anyhow::Result<u32> {
    Ok(get_active_session_uid_and_class().await?.0)
}

pub async fn get_active_session_uid_and_class() -> anyhow::Result<(u32, String)> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1/seat/seat0",
        "org.freedesktop.login1.Seat",
    )
    .await?;
    let active_session: (String, zbus::zvariant::ObjectPath) =
        proxy.get_property("ActiveSession").await?;

    let session_proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        active_session.1,
        "org.freedesktop.login1.Session",
    )
    .await?;
    let user: (u32, zbus::zvariant::ObjectPath) = session_proxy.get_property("User").await?;
    let class: String = session_proxy.get_property("Class").await?;

    Ok((user.0, class))
}

#[proxy(
    interface = "com.gundulabs.Gaze",
    default_service = "com.gundulabs.Gaze",
    default_path = "/com/gundulabs/Gaze"
)]
pub trait Gaze {
    async fn claim(&self, username: &str) -> zbus::Result<()>;
    async fn release(&self) -> zbus::Result<()>;

    async fn register_extension(&self, active: bool) -> zbus::Result<()>;
    async fn is_extension_active(&self, uid: u32) -> zbus::Result<bool>;

    async fn verify_start(&self, face_name: &str) -> zbus::Result<()>;
    async fn verify_start_for(&self, face_name: &str, pam_service: &str) -> zbus::Result<()>;
    async fn verify_stop(&self) -> zbus::Result<()>;

    async fn enroll_start(&self, face_name: &str) -> zbus::Result<()>;
    async fn enroll_stop(&self) -> zbus::Result<()>;

    async fn list_faces(&self, username: &str) -> zbus::Result<Vec<(String, u32, bool, bool)>>;
    async fn has_enrolled_faces(&self, username: &str) -> zbus::Result<bool>;
    async fn is_camera_available(&self) -> zbus::Result<bool>;
    async fn benchmark(&self) -> zbus::Result<Vec<BenchmarkResult>>;
    async fn delete_face(&self, username: &str, face_name: &str) -> zbus::Result<bool>;
    async fn rename_face(
        &self,
        username: &str,
        old_face_name: &str,
        new_face_name: &str,
    ) -> zbus::Result<bool>;
    async fn delete_faces(&self, username: &str) -> zbus::Result<bool>;

    #[zbus(property)]
    fn config(&self) -> zbus::Result<Config>;

    #[zbus(property)]
    fn set_config(&self, value: Config) -> zbus::Result<()>;

    async fn get_gdm_face_auth(&self) -> zbus::Result<bool>;
    #[zbus(allow_interactive_auth)]
    async fn set_gdm_face_auth(&self, enabled: bool) -> zbus::Result<bool>;

    #[zbus(signal)]
    fn face_status(&self, status: CaptureStatus) -> zbus::Result<()>;

    #[zbus(signal)]
    fn verify_status(
        &self,
        result: VerifyResult,
        faces: Vec<(String, f64, f64, bool, f64, f64, bool)>,
        rgb_status: CaptureStatus,
        ir_status: CaptureStatus,
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

    #[derive(Clone, Debug, Value, OwnedValue, Type)]
    struct OldBenchmarkResult {
        component: String,
        mean_ms: f64,
        p95_ms: f64,
        min_ms: f64,
        fps: f64,
    }

    fn benchmark_result(requested_device: &str, fallback_reason: &str) -> BenchmarkResult {
        BenchmarkResult {
            component: "Face detector".to_string(),
            execution_provider: "cpu".to_string(),
            device: "cpu".to_string(),
            requested_execution_provider: "cpu".to_string(),
            requested_device: requested_device.to_string(),
            fallback_reason: fallback_reason.to_string(),
            mean_ms: 1.0,
            p95_ms: 2.0,
            min_ms: 0.5,
            fps: 1000.0,
        }
    }

    #[test]
    fn current_benchmark_layout_decodes() {
        let raw = OwnedValue::try_from(Value::from(vec![benchmark_result("cpu", "")])).unwrap();
        let decoded = benchmark_from_reply(raw)
            .expect("no error")
            .expect("current layout is readable");

        assert_eq!(decoded.len(), 1);
        assert!(decoded[0].ran_as_configured());
    }

    #[test]
    fn older_daemon_benchmark_layout_is_reported_not_decoded() {
        let old = vec![OldBenchmarkResult {
            component: "Face detector".to_string(),
            mean_ms: 1.0,
            p95_ms: 2.0,
            min_ms: 0.5,
            fps: 1000.0,
        }];
        let raw = OwnedValue::try_from(Value::from(old)).expect("old results convert to a value");

        assert!(
            benchmark_from_reply(raw)
                .expect("a layout mismatch is not an error")
                .is_none()
        );
    }

    #[test]
    fn a_device_fallback_is_not_reported_as_configured() {
        assert!(!benchmark_result("npu", "no npu driver").ran_as_configured());
        assert!(!benchmark_result("npu", "").ran_as_configured());
    }

    #[test]
    fn enum_display_strings_are_user_facing_messages() {
        assert_eq!(
            CaptureStatus::NoFace.to_string(),
            "Please look at the camera..."
        );
        assert_eq!(CaptureStatus::TooDark.to_string(), "Need more light...");
        assert_eq!(CaptureStatus::Ready.to_string(), "Hold still...");
        assert_eq!(CaptureStatus::Usable.to_string(), "Hold still...");
        assert_eq!(
            EnrollPrompt::LookLeft.to_string(),
            "Turn your face slightly left"
        );
        assert_eq!(VerifyResult::VerifyNoMatch.as_ref(), "VerifyNoMatch");
    }

    #[derive(Clone, Debug, Value, OwnedValue, Type)]
    struct OldConfig {
        security: crate::config::SecurityLevel,
        cameras: crate::config::CameraConfig,
        auth: crate::config::AuthConfig,
        enrollment: crate::config::EnrollmentConfig,
        liveness: crate::config::LivenessConfig,
        storage: crate::config::StorageConfig,
    }

    fn old_daemon_property() -> OwnedValue {
        let old = OldConfig {
            security: Default::default(),
            cameras: Default::default(),
            auth: Default::default(),
            enrollment: Default::default(),
            liveness: Default::default(),
            storage: Default::default(),
        };
        OwnedValue::try_from(Value::from(old)).expect("old config converts to a value")
    }

    #[test]
    fn current_layout_decodes() {
        let raw = OwnedValue::try_from(Value::from(Config::default())).unwrap();
        let decoded = config_from_property(raw)
            .expect("no error")
            .expect("current layout is readable");
        assert_eq!(decoded.auth.start_delay_scope(), "all");
    }

    #[test]
    fn older_daemon_layout_is_reported_not_decoded() {
        assert!(
            config_from_property(old_daemon_property())
                .expect("a layout mismatch is not an error")
                .is_none()
        );
    }

    #[test]
    fn decoding_an_older_layout_directly_would_panic() {
        let raw = old_daemon_property();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Config::try_from(raw).is_ok()
        }));
        std::panic::set_hook(previous);
        assert!(
            !matches!(attempt, Ok(true)),
            "expected the raw conversion to fail loudly on a short structure"
        );
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
            serde_plain::to_string(&CaptureStatus::Ready).unwrap(),
            "ready"
        );
        assert_eq!(
            serde_plain::to_string(&CaptureStatus::Usable).unwrap(),
            "usable"
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

        let err = zbus::Error::Failure(
            "org.freedesktop.DBus.Error.ServiceUnknown: service is not activatable".to_string(),
        );
        assert!(dbus_is_not_activatable(&err));

        let err = zbus::Error::Failure("ServiceUnknown".to_string());
        assert!(dbus_is_not_activatable(&err));

        let err = zbus::Error::Failure("camera unavailable".to_string());
        assert!(!dbus_is_not_activatable(&err));
    }
}
