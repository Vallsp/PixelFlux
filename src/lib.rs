//! Collaborative pixel canvas — core application logic.
//!
//! A shared `WIDTH x HEIGHT` grid where each cell holds a palette index
//! (0..15), encoded as one hex character. The whole canvas is a single
//! `WIDTH*HEIGHT` string.
//!
//! State lives in Redis when a reachable `REDIS_URL` is provided (shared
//! across instances and visitors), otherwise in an in-process canvas so the
//! app still runs with zero dependencies.
//!
//! Live updates: every painted pixel is published to a Redis pub/sub channel,
//! and each instance subscribes to that channel and fans messages out to its
//! own browsers over Server-Sent Events. This makes real-time work across
//! every replica behind a load balancer. Without Redis, the broadcast stays
//! in-process.

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

pub const WIDTH: usize = 64;
pub const HEIGHT: usize = 64;
const CANVAS_KEY: &str = "canvas";
const EVENTS_CHANNEL: &str = "canvas:events";

/// 16-colour palette (index = hex char stored per pixel).
pub const PALETTE: [&str; 16] = [
    "#ffffff", "#e4e4e4", "#888888", "#222222", "#ffa7d1", "#e50000", "#e59500", "#a06a42",
    "#e5d900", "#94e044", "#02be01", "#00d3dd", "#0083c7", "#0000ea", "#cf6ee4", "#820080",
];

#[derive(Debug, thiserror::Error)]
pub enum PixelError {
    #[error("coordinates ({x}, {y}) out of bounds ({width}×{height})")]
    OutOfBounds {
        x: usize,
        y: usize,
        width: usize,
        height: usize,
    },
    #[error("color {0} is not in the palette (0–15)")]
    InvalidColor(u8),
}

/// Map a `PixelError` to a 400 response carrying its message.
impl IntoResponse for PixelError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                ok: false,
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

fn blank_canvas() -> Vec<u8> {
    vec![b'0'; WIDTH * HEIGHT]
}

fn hex_char(color: u8) -> Option<u8> {
    std::char::from_digit(color as u32, 16).map(|c| c as u8)
}

/// Background task: subscribe to the Redis events channel and forward every
/// published pixel into the local broadcast channel (which feeds SSE clients).
/// Reconnects on failure. This is what makes real-time updates work across
/// multiple instances.
fn spawn_event_subscriber(client: redis::Client, tx: broadcast::Sender<String>) {
    tokio::spawn(async move {
        loop {
            if let Ok(mut pubsub) = client.get_async_pubsub().await {
                if pubsub.subscribe(EVENTS_CHANNEL).await.is_ok() {
                    let mut stream = pubsub.on_message();
                    while let Some(msg) = stream.next().await {
                        if let Ok(payload) = msg.get_payload::<String>() {
                            let _ = tx.send(payload);
                        }
                    }
                }
            }
            // Connection dropped or failed: back off, then retry.
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    redis: Option<redis::aio::ConnectionManager>,
    fallback: Arc<Mutex<Vec<u8>>>,
    tx: broadcast::Sender<String>,
}

impl AppState {
    /// Build the application state.
    ///
    /// If `redis_url` is `Some` and reachable, the canvas is backed by Redis
    /// (initialised once, atomically, if absent) and a pub/sub subscriber is
    /// started for cross-instance live updates. Any connection error degrades
    /// gracefully to the in-memory canvas with in-process broadcast.
    pub async fn new(redis_url: Option<String>) -> Self {
        let (tx, _rx) = broadcast::channel(1024);

        let mut redis = None;
        if let Some(url) = redis_url {
            if let Ok(client) = redis::Client::open(url) {
                if let Ok(conn) = redis::aio::ConnectionManager::new(client.clone()).await {
                    let mut init = conn.clone();
                    let blank = String::from_utf8(blank_canvas()).unwrap();
                    let _ = redis::cmd("SETNX")
                        .arg(CANVAS_KEY)
                        .arg(blank)
                        .query_async::<i64>(&mut init)
                        .await;
                    spawn_event_subscriber(client, tx.clone());
                    redis = Some(conn);
                }
            }
        }

        Self {
            redis,
            fallback: Arc::new(Mutex::new(blank_canvas())),
            tx,
        }
    }

    /// Return the whole canvas as a `WIDTH*HEIGHT` hex string.
    pub async fn canvas(&self) -> String {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            if let Ok(Some(s)) = redis::cmd("GET")
                .arg(CANVAS_KEY)
                .query_async::<Option<String>>(&mut conn)
                .await
            {
                if s.len() == WIDTH * HEIGHT {
                    return s;
                }
            }
        }
        String::from_utf8(self.fallback.lock().unwrap().clone()).unwrap()
    }

    /// Paint a single pixel. Returns an error if the coordinates or colour are
    /// invalid. With Redis the update is published to every instance; otherwise
    /// it is broadcast in-process.
    pub async fn set_pixel(&self, x: usize, y: usize, color: u8) -> Result<(), PixelError> {
        if x >= WIDTH || y >= HEIGHT {
            return Err(PixelError::OutOfBounds {
                x,
                y,
                width: WIDTH,
                height: HEIGHT,
            });
        }
        let ch = hex_char(color).ok_or(PixelError::InvalidColor(color))?;
        let offset = y * WIDTH + x;
        let payload = format!(r#"{{"x":{x},"y":{y},"color":{color}}}"#);

        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let wrote = redis::cmd("SETRANGE")
                .arg(CANVAS_KEY)
                .arg(offset)
                .arg((ch as char).to_string())
                .query_async::<i64>(&mut conn)
                .await
                .is_ok();
            if wrote {
                // Fan out to every instance (including this one via its subscriber).
                let _ = redis::cmd("PUBLISH")
                    .arg(EVENTS_CHANNEL)
                    .arg(&payload)
                    .query_async::<i64>(&mut conn)
                    .await;
                return Ok(());
            }
        }

        self.fallback.lock().unwrap()[offset] = ch;
        let _ = self.tx.send(payload);
        Ok(())
    }

    /// Subscribe to the live pixel event stream (the same feed used by SSE).
    /// Each message is the JSON payload `{"x":..,"y":..,"color":..}`.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// A stream of live pixel events for SSE subscribers.
    fn events(&self) -> impl Stream<Item = Result<Event, Infallible>> {
        BroadcastStream::new(self.tx.subscribe()).filter_map(|msg| match msg {
            Ok(json) => Some(Ok(Event::default().data(json))),
            Err(_) => None, // lagged: client will resync via full fetch
        })
    }
}

#[derive(Serialize)]
struct Health {
    status: &'static str,
}

#[derive(Serialize)]
struct Info {
    name: &'static str,
    version: &'static str,
    instance: String,
}

/// Identify the running instance. In Kubernetes (and Docker) `HOSTNAME` is the
/// pod / container name, which makes load balancing visible in the UI.
fn instance_id() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Serialize)]
struct CanvasResponse {
    width: usize,
    height: usize,
    palette: [&'static str; 16],
    pixels: String,
}

#[derive(Deserialize)]
struct PixelRequest {
    x: usize,
    y: usize,
    color: u8,
}

#[derive(Serialize)]
struct PixelResponse {
    ok: bool,
}

#[derive(Serialize)]
struct ErrorResponse {
    ok: bool,
    error: String,
}

async fn index() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn info() -> Json<Info> {
    Json(Info {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        instance: instance_id(),
    })
}

async fn get_canvas(State(state): State<AppState>) -> Json<CanvasResponse> {
    Json(CanvasResponse {
        width: WIDTH,
        height: HEIGHT,
        palette: PALETTE,
        pixels: state.canvas().await,
    })
}

async fn put_pixel(
    State(state): State<AppState>,
    Json(req): Json<PixelRequest>,
) -> Result<Json<PixelResponse>, PixelError> {
    state.set_pixel(req.x, req.y, req.color).await?;
    Ok(Json(PixelResponse { ok: true }))
}

async fn sse_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(state.events()).keep_alive(KeepAlive::default())
}

/// Build the application router.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/api/canvas", get(get_canvas))
        .route("/api/pixel", post(put_pixel))
        .route("/api/events", get(sse_events))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    async fn body_string(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn canvas_starts_blank() {
        let state = AppState::new(None).await;
        let canvas = state.canvas().await;
        assert_eq!(canvas.len(), WIDTH * HEIGHT);
        assert!(canvas.chars().all(|c| c == '0'));
    }

    #[tokio::test]
    async fn set_pixel_updates_the_canvas() {
        let state = AppState::new(None).await;
        assert!(state.set_pixel(1, 0, 5).await.is_ok()); // offset 1 -> '5'
        assert!(state.set_pixel(0, 1, 10).await.is_ok()); // offset WIDTH -> 'a'

        let canvas = state.canvas().await;
        assert_eq!(canvas.as_bytes()[1], b'5');
        assert_eq!(canvas.as_bytes()[WIDTH], b'a');
    }

    #[tokio::test]
    async fn set_pixel_rejects_invalid_input() {
        let state = AppState::new(None).await;
        assert!(state.set_pixel(WIDTH, 0, 1).await.is_err()); // x out of bounds
        assert!(state.set_pixel(0, HEIGHT, 1).await.is_err()); // y out of bounds
        assert!(state.set_pixel(0, 0, 99).await.is_err()); // colour out of palette
    }

    #[tokio::test]
    async fn painting_emits_a_live_event() {
        let state = AppState::new(None).await;
        let mut rx = state.tx.subscribe();
        assert!(state.set_pixel(3, 4, 7).await.is_ok());
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg, r#"{"x":3,"y":4,"color":7}"#);
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let response = app(AppState::new(None).await)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, r#"{"status":"ok"}"#);
    }

    #[tokio::test]
    async fn canvas_endpoint_exposes_dimensions() {
        let response = app(AppState::new(None).await)
            .oneshot(
                Request::builder()
                    .uri("/api/canvas")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains(r#""width":64"#), "got: {body}");
        assert!(body.contains(r#""height":64"#), "got: {body}");
    }
}
