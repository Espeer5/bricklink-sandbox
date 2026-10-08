use clap::Parser;
use std::net::SocketAddr;

/// Run an unofficial, in-memory BrickLink Store API sandbox.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Bind address. Localhost is the default; authentication is not enforced.
    #[arg(long, default_value = "127.0.0.1:8000")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!(
        "BrickLink sandbox: http://{}/api/store/v1",
        listener.local_addr()?
    );
    axum::serve(listener, bricklink_sandbox::app())
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
