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
    let pool = db::init_pool(&config.database_url).await?;
    let port = config.port;
    let state = AppState::new(pool, config);

    // Restore the persisted realtime gateway configuration (CRD 3289-3292).
    mcss_backend::realtime::endpoints::hydrate_config(&state).await;

    // Delayed-message dispatcher: fires scheduled sends when they become due
    // (CRD 991) and periodically retires stale failed items (CRD 1028).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticks: u64 = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                mcss_backend::domain::messaging::service::dispatch_due(&state).await;
                ticks += 1;
                // Scheduled reminder processing (CRD 5048): once a minute.
                if ticks.is_multiple_of(60) {
                    let _ =
                        mcss_backend::domain::notifications::reminders::process_due(&state).await;
                }
                if ticks.is_multiple_of(3600) {
                    let _ =
                        mcss_backend::domain::messaging::service::archive_stale_failed(&state.db)
                            .await;
                }
            }
        });
    }

    mcss_backend::domain::queue::worker::spawn(state.clone());
    let router = app::build_router(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("MCSS backend listening on port {port}");
    axum::serve(listener, router).await?;
    Ok(())
}
