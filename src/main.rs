use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

/// Run an unofficial, in-memory BrickLink Store API sandbox.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Bind address. Localhost is the default; authentication is not enforced.
    #[arg(long, default_value = "127.0.0.1:8000")]
    bind: SocketAddr,
    /// Version-1 JSON fixture with inventory, historical orders, clock, and steps.
    #[arg(long)]
    fixture: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let app = if let Some(path) = args.fixture {
        let contents =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        bricklink_sandbox::app_from_fixture(&contents)
            .map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        bricklink_sandbox::app()
    };
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!(
        "BrickLink sandbox: http://{}/api/store/v1",
        listener.local_addr()?
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
