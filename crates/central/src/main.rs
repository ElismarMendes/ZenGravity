//! Binário standalone da Central (`zengravity-central.exe`).

use zengravity_central::spawn_central_with_simulator;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,zengravity=debug".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let bind = args
        .windows(2)
        .find(|w| w[0] == "--bind")
        .map(|w| w[1].as_str())
        .unwrap_or("127.0.0.1:8790");

    let handle = spawn_central_with_simulator(bind, "data/zengravity.db", 20261118).await?;
    tracing::info!(
        ws_url = %handle.ws_url,
        "ZenGravity Central ativa (Modo Simulacao Reproduzivel). Pressione Ctrl+C para encerrar."
    );

    tokio::signal::ctrl_c().await?;
    Ok(())
}
