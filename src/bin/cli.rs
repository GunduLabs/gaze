use clap::{Parser, Subcommand};
use zbus::Connection;
use zbus::proxy;

#[proxy(
    interface = "org.gaze.Auth",
    default_service = "org.gaze.Auth",
    default_path = "/org/gaze/Auth"
)]
trait Auth {
    async fn authenticate(&self, username: &str, image_path: &str) -> zbus::Result<bool>;
    async fn add_face(&self, username: &str, image_path: &str) -> zbus::Result<String>;
    async fn remove_face(&self, username: &str, uuid: &str) -> zbus::Result<bool>;
    async fn clear_user(&self, username: &str) -> zbus::Result<bool>;
}

#[derive(Parser)]
#[command(name = "gaze", about = "Gaze Facial Authentication CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Auth {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        image: String,
    },
    AddFace {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        image: String,
    },
    RemoveFace {
        #[arg(short, long)]
        user: String,
        #[arg(long)]
        uuid: String,
    },
    ClearUser {
        #[arg(short, long)]
        user: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let conn = Connection::system().await?;
    let proxy = AuthProxy::new(&conn).await?;

    match cli.command {
        Commands::Auth { user, image } => {
            let result = proxy.authenticate(&user, &image).await?;
            if result {
                println!("Authenticated!");
            } else {
                println!("Access Denied.");
            }
        }
        Commands::AddFace { user, image } => {
            let uuid = proxy.add_face(&user, &image).await?;
            println!("Face added for '{}' (uuid: {})", user, uuid);
        }
        Commands::RemoveFace { user, uuid } => {
            let removed = proxy.remove_face(&user, &uuid).await?;
            if removed {
                println!("Face '{}' removed for '{}'", uuid, user);
            } else {
                println!("Face '{}' not found for '{}'", uuid, user);
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
