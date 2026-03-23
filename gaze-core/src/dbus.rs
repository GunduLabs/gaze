use crate::config::Config;
use zbus::proxy;

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

pub async fn load_config_from_daemon(proxy: &AuthProxy<'_>) -> anyhow::Result<Config> {
    let config_toml = proxy.get_config_toml().await?;
    let config: Config = toml::from_str(&config_toml)?;
    Ok(config)
}

pub async fn apply_config_to_daemon(proxy: &AuthProxy<'_>, config: &Config) -> anyhow::Result<()> {
    let encoded = toml::to_string_pretty(config)?;
    proxy.set_config_toml(&encoded).await?;
    Ok(())
}

#[proxy(
    interface = "org.gaze.Auth",
    default_service = "org.gaze.Auth",
    default_path = "/org/gaze/Auth"
)]
pub trait Auth {
    async fn verify(
        &self,
        username: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> zbus::Result<bool>;

    #[allow(clippy::type_complexity)]
    async fn match_faces(
        &self,
        username: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> zbus::Result<Vec<(String, f64, f64, bool, u32)>>;

    async fn add_face(
        &self,
        username: &str,
        face_name: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> zbus::Result<String>;

    async fn list_faces(&self, username: &str) -> zbus::Result<Vec<(String, u32)>>;
    async fn remove_face(&self, username: &str, face_name: &str) -> zbus::Result<bool>;
    async fn rename_face(
        &self,
        username: &str,
        old_face_name: &str,
        new_face_name: &str,
    ) -> zbus::Result<bool>;
    async fn clear_user(&self, username: &str) -> zbus::Result<bool>;

    async fn get_config_toml(&self) -> zbus::Result<String>;
    async fn set_config_toml(&self, config_toml: &str) -> zbus::Result<bool>;
}
