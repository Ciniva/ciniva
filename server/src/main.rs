use ciniva::{api, config::Config, library};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use std::{str::FromStr, time::Duration};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "ciniva=info,tower_http=warn".into()))
        .init();

    let cfg = Config::from_env();
    tokio::fs::create_dir_all(&cfg.data_dir).await?;
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", cfg.data_dir.join("ciniva.db").display()))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(10));
    let db = SqlitePoolOptions::new().max_connections(8).connect_with(opts).await?;
    sqlx::migrate!().run(&db).await?;

    if cfg.tmdb_key.is_none() {
        tracing::warn!("TMDB_API_KEY is not set: files will be scanned and probed but not matched");
    }
    tracing::info!(media = ?cfg.media_dirs, data = %cfg.data_dir.display(), "starting Ciniva");

    let ctx = library::Ctx::new(db, cfg.clone());
    let scanner = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(scanner.cfg.scan_interval_mins.max(1) * 60));
        loop {
            tick.tick().await;
            if let Err(e) = library::scan(&scanner).await {
                tracing::error!("scan failed: {e:#}");
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.port)).await?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, api::router(ctx)).with_graceful_shutdown(async { let _ = tokio::signal::ctrl_c().await; }).await?;
    Ok(())
}
