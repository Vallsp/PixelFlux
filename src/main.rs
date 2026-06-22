use std::net::SocketAddr;
use tokio::signal;
use pixelflux::{app, AppState};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().json().init();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse()
        .expect("PORT must be a valid number");
    let redis_url = std::env::var("REDIS_URL").ok();

    let state = AppState::new(redis_url).await;
    let router = app(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "server starting");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("shutting down");
}
