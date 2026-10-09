use clap::Parser;
use std::{net::SocketAddr, path::PathBuf};

/// Run an unofficial, in-memory BrickLink Store API sandbox.
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Bind address. Keep this local: mock controls are intentionally unauthenticated.
    #[arg(long, default_value = "127.0.0.1:8000")]
    bind: SocketAddr,
    /// Version-1 JSON fixture with inventory, historical orders, clock, and steps.
    #[arg(long)]
    fixture: Option<PathBuf>,
    /// Optional catalog fixture, replacing an embedded catalog.
    #[arg(long)]
    catalog: Option<PathBuf>,
    /// Optional dummy OAuth and fault configuration. Never supply live credentials.
    #[arg(long)]
    validation: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let read = |path: Option<PathBuf>| -> Result<Option<String>, Box<dyn std::error::Error>> {
        path.map(|p| {
            std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()).into())
        })
        .transpose()
    };
    let source_paths = [&args.fixture, &args.catalog, &args.validation]
        .into_iter()
        .flatten()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let fixture = read(args.fixture)?;
    let catalog = read(args.catalog)?;
    let validation = read(args.validation)?;
    let app = bricklink_sandbox::app_configured(
        fixture.as_deref(),
        catalog.as_deref(),
        validation.as_deref(),
    )
    .map_err(|e| format!("{source_paths}: {e}"))?;
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!(
        "BrickLink sandbox: http://{}/api/store/v1",
        listener.local_addr()?
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}
