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

    tracing::info!(media = ?cfg.media_dirs, data = %cfg.data_dir.display(), "starting Ciniva");

    let hw = std::sync::Arc::new(ciniva::hwaccel::detect(&cfg.ffmpeg, &cfg.data_dir, &cfg.hwaccel).await?);
    let ctx = library::Ctx::new(db, cfg.clone(), hw);
    if let Some(key) = ciniva::auth::setting(&ctx.db, "tmdb_api_key").await {
        ctx.set_tmdb_key(Some(key)).await;
    }
    if ctx.tmdb().await.is_none() {
        tracing::warn!("no TMDB key yet: files are scanned and probed but not matched until one is set in setup");
    }
    if ciniva::auth::user_count(&ctx.db).await? == 0 {
        tracing::warn!("no accounts yet: open the web UI to create the administrator");
    }
    match library::sweep_stale_caches(&ctx).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(files = n, "removed caches left over from deleted or reused file ids"),
        Err(e) => tracing::warn!("cache sweep failed: {e:#}"),
    }
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
