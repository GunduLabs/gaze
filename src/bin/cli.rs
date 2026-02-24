use clap::{Parser, Subcommand};
use gaze_core::camera::Camera;
use gaze_core::config::Config;
use opencv::prelude::*;
use zbus::Connection;
use zbus::proxy;

#[proxy(
    interface = "org.gaze.Auth",
    default_service = "org.gaze.Auth",
    default_path = "/org/gaze/Auth"
)]
trait Auth {
    async fn authenticate(
        &self,
        username: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> zbus::Result<bool>;

    async fn add_face(
        &self,
        username: &str,
        face_name: &str,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> zbus::Result<String>;

    async fn remove_face(&self, username: &str, face_name: &str) -> zbus::Result<bool>;
    async fn clear_user(&self, username: &str) -> zbus::Result<bool>;
}

fn capture_frame_bytes(config: &Config) -> anyhow::Result<(Vec<u8>, u32, u32)> {
    let mut cam = Camera::open(&config.cameras.rgb)?;
    let frame = cam.capture_frame()?;
    let sz = frame.size()?;
    let total_bytes = (sz.width * sz.height * 3) as usize;
    let mut bytes = vec![0u8; total_bytes];
    unsafe {
        std::ptr::copy_nonoverlapping(frame.data(), bytes.as_mut_ptr(), total_bytes);
    }
    Ok((bytes, sz.width as u32, sz.height as u32))
}

#[derive(Parser)]
#[command(name = "gaze", about = "Gaze Facial Authentication CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate a user via webcam capture
    Auth {
        #[arg(short, long)]
        user: String,
    },
    /// Capture a face from webcam and store under a named face
    AddFace {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        face: String,
    },
    /// Remove all embeddings for a named face
    RemoveFace {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        face: String,
    },
    /// Clear all faces and embeddings for a user
    ClearUser {
        #[arg(short, long)]
        user: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let conn = Connection::system().await?;
    let proxy = AuthProxy::new(&conn).await?;

    match cli.command {
        Commands::Auth { user } => {
            let (bytes, width, height) = capture_frame_bytes(&config)?;
            let result = proxy.authenticate(&user, &bytes, width, height).await?;
            if result {
                println!("Authenticated!");
            } else {
                println!("Access Denied.");
            }
        }
        Commands::AddFace { user, face } => {
            let (bytes, width, height) = capture_frame_bytes(&config)?;
            let uuid = proxy.add_face(&user, &face, &bytes, width, height).await?;
            println!("Embedding added to '{}/{}' (uuid: {})", user, face, uuid);
        }
        Commands::RemoveFace { user, face } => {
            let removed = proxy.remove_face(&user, &face).await?;
            if removed {
                println!("Face '{}' removed for '{}'", face, user);
            } else {
                println!("Face '{}' not found for '{}'", face, user);
            }
        }
        Commands::ClearUser { user } => {
            let cleared = proxy.clear_user(&user).await?;
            if cleared {
                println!("All faces cleared for '{}'", user);
            } else {
                println!("No faces found for '{}'", user);
            }
        }
    }

    Ok(())
}
