use mcss_backend::{app, config::Config, db, state::AppState};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let config = Config::from_env();
    if let Some(dir) = config
        .database_url
        .strip_prefix("sqlite://")
        .and_then(|p| std::path::Path::new(p.split('?').next().unwrap_or(p)).parent())
    {
        std::fs::create_dir_all(dir).ok();
    }
    let pool = db::init_pool(&config.database_url).await?;
    let port = config.port;
    let state = AppState::new(pool, config);
    let router = app::build_router(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("MCSS backend listening on port {port}");
    axum::serve(listener, router).await?;
    Ok(())
}
