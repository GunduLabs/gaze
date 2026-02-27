use zbus::proxy;

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
    async fn clear_user(&self, username: &str) -> zbus::Result<bool>;
}
