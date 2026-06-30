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
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

pub const WIDTH: usize = 200;
pub const HEIGHT: usize = 200;
const CANVAS_KEY: &str = "canvas";
const EVENTS_CHANNEL: &str = "canvas:events";

/// Registration is deliberately slow so minting tokens en masse is expensive
/// (you can't just create a fresh user on every paint request).
const REGISTER_DELAY: Duration = Duration::from_secs(5);
/// How long an issued token stays valid server-side (clients re-register on 401).
const TOKEN_TTL_SECS: u64 = 86_400;
/// Per-token paint budget within a rolling window.
const RATE_LIMIT: u64 = 4096;
const RATE_WINDOW_SECS: u64 = 30;

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

/// Errors returned by the paint endpoint: invalid pixel (400), missing/unknown
/// token (401), or too many pixels in the window (429).
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Pixel(#[from] PixelError),
    #[error("missing or unknown token — register first")]
    Unauthorized,
    #[error("rate limit exceeded — max 4096 pixels per 30s per token")]
    RateLimited,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            ApiError::Pixel(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        };
        (
            status,
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

/// Pending pixel updates, keyed by `(x, y)` so a cell repainted within the same
/// window keeps only its last colour. Flushed to SSE clients on a fixed tick.
type PixelBuffer = Arc<Mutex<HashMap<(u16, u16), u8>>>;

/// One pixel update — the unit of the Redis pub/sub payload and of each entry
/// in a batched SSE event.
#[derive(Serialize, Deserialize)]
struct PixelEvent {
    x: u16,
    y: u16,
    color: u8,
}

/// Drain a buffer into one batched SSE payload: a JSON array of pixel updates,
/// or `None` if empty. Pure (no timing or I/O) so it is unit-testable directly.
fn drain_to_batch(map: HashMap<(u16, u16), u8>) -> Option<String> {
    if map.is_empty() {
        return None;
    }
    let events: Vec<PixelEvent> = map
        .into_iter()
        .map(|((x, y), color)| PixelEvent { x, y, color })
        .collect();
    serde_json::to_string(&events).ok()
}

/// Background task: every `period`, swap the buffer out and, if non-empty,
/// broadcast a single batched event to all SSE clients. Coalescing the fan-out
/// onto a tick turns its cost from `writes × clients` into `ticks × clients`.
fn spawn_flusher(buffer: PixelBuffer, tx: broadcast::Sender<String>, period: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(period);
        loop {
            tick.tick().await;
            // Swap the buffer out and drop the lock *before* serialising/sending.
            let drained = std::mem::take(&mut *buffer.lock().unwrap());
            if let Some(batch) = drain_to_batch(drained) {
                let _ = tx.send(batch);
            }
        }
    });
}

/// Background task: subscribe to the Redis events channel and fold every
/// published pixel into the shared buffer (which the flusher fans out). This is
/// what makes real-time updates work across multiple instances. Reconnects on
/// failure.
fn spawn_event_subscriber(client: redis::Client, buffer: PixelBuffer) {
    tokio::spawn(async move {
        loop {
            if let Ok(mut pubsub) = client.get_async_pubsub().await {
                if pubsub.subscribe(EVENTS_CHANNEL).await.is_ok() {
                    let mut stream = pubsub.on_message();
                    while let Some(msg) = stream.next().await {
                        if let Ok(payload) = msg.get_payload::<String>() {
                            if let Ok(ev) = serde_json::from_str::<PixelEvent>(&payload) {
                                buffer.lock().unwrap().insert((ev.x, ev.y), ev.color);
                            }
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
    /// Pending pixel updates, flushed to SSE clients on a tick (coalesced).
    buffer: PixelBuffer,
    /// Issued tokens (used only when Redis is unavailable).
    tokens: Arc<Mutex<HashSet<String>>>,
    /// Per-token paint counts + window start (fallback rate limiter).
    rates: Arc<Mutex<HashMap<String, (u64, Instant)>>>,
    /// Live-viewer count (used only when Redis is unavailable).
    online: Arc<AtomicI64>,
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
        let buffer: PixelBuffer = Arc::new(Mutex::new(HashMap::new()));

        // Coalesce the SSE fan-out onto a fixed tick (default 16ms; lower it via
        // SSE_COALESCE_MS in tests to flush almost immediately).
        let coalesce_ms = std::env::var("SSE_COALESCE_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(16);
        spawn_flusher(
            buffer.clone(),
            tx.clone(),
            Duration::from_millis(coalesce_ms),
        );

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
                    spawn_event_subscriber(client, buffer.clone());
                    redis = Some(conn);
                }
            }
        }

        Self {
            redis,
            fallback: Arc::new(Mutex::new(blank_canvas())),
            tx,
            buffer,
            tokens: Arc::new(Mutex::new(HashSet::new())),
            rates: Arc::new(Mutex::new(HashMap::new())),
            online: Arc::new(AtomicI64::new(0)),
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
        // Feed the coalescing buffer; the flusher batches and broadcasts it.
        self.buffer
            .lock()
            .unwrap()
            .insert((x as u16, y as u16), color);
        Ok(())
    }

    /// Persist a token (Redis when available so every replica accepts it,
    /// otherwise in-process). No artificial delay — see `register_token`.
    async fn store_token(&self, token: &str) {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("SET")
                .arg(format!("token:{token}"))
                .arg(1)
                .arg("EX")
                .arg(TOKEN_TTL_SECS)
                .query_async::<String>(&mut conn)
                .await;
        } else {
            self.tokens.lock().unwrap().insert(token.to_string());
        }
    }

    /// Issue a fresh random token. Deliberately slow (`REGISTER_DELAY`) so a
    /// client can't cheaply mint a new identity on every paint request.
    pub async fn register_token(&self) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        tokio::time::sleep(REGISTER_DELAY).await;
        self.store_token(&token).await;
        token
    }

    /// Validate `token` and count this paint against its budget. Returns
    /// `Unauthorized` for an unknown token and `RateLimited` past the window
    /// limit. Backed by Redis (shared across replicas) or an in-process store.
    pub async fn authorize(&self, token: &str) -> Result<(), ApiError> {
        if token.is_empty() {
            return Err(ApiError::Unauthorized);
        }
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let known: bool = redis::cmd("EXISTS")
                .arg(format!("token:{token}"))
                .query_async(&mut conn)
                .await
                .unwrap_or(false);
            if !known {
                return Err(ApiError::Unauthorized);
            }
            let key = format!("rate:{token}");
            let count: i64 = redis::cmd("INCR")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .unwrap_or(1);
            if count == 1 {
                let _ = redis::cmd("EXPIRE")
                    .arg(&key)
                    .arg(RATE_WINDOW_SECS)
                    .query_async::<i64>(&mut conn)
                    .await;
            }
            if count as u64 > RATE_LIMIT {
                return Err(ApiError::RateLimited);
            }
        } else {
            if !self.tokens.lock().unwrap().contains(token) {
                return Err(ApiError::Unauthorized);
            }
            let mut rates = self.rates.lock().unwrap();
            let now = Instant::now();
            let entry = rates.entry(token.to_string()).or_insert((0, now));
            if now.duration_since(entry.1).as_secs() >= RATE_WINDOW_SECS {
                *entry = (0, now);
            }
            entry.0 += 1;
            if entry.0 > RATE_LIMIT {
                return Err(ApiError::RateLimited);
            }
        }
        Ok(())
    }

    /// Count of currently connected viewers (open SSE streams). Shared across
    /// replicas via Redis, or per-instance in the fallback.
    async fn online_add(&self, delta: i64) {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let cmd = if delta >= 0 { "INCRBY" } else { "DECRBY" };
            let _ = redis::cmd(cmd)
                .arg("online")
                .arg(delta.abs())
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            self.online.fetch_add(delta, Ordering::Relaxed);
        }
    }

    /// Current number of connected viewers (never negative).
    pub async fn online(&self) -> i64 {
        let n = if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            redis::cmd("GET")
                .arg("online")
                .query_async::<Option<i64>>(&mut conn)
                .await
                .ok()
                .flatten()
                .unwrap_or(0)
        } else {
            self.online.load(Ordering::Relaxed)
        };
        n.max(0)
    }

    /// Subscribe to the live pixel event stream (the same feed used by SSE).
    /// Each message is a coalesced batch: a JSON array
    /// `[{"x":..,"y":..,"color":..}, ...]`.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// A stream of batched live pixel events for SSE subscribers (one event per
    /// flush tick, each carrying a JSON array of updates).
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
    online: i64,
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
struct RegisterResponse {
    token: String,
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

async fn info(State(state): State<AppState>) -> Json<Info> {
    Json(Info {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        instance: instance_id(),
        online: state.online().await,
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

/// Issue a token. Slow on purpose (anti-abuse) — see `register_token`.
async fn register(State(state): State<AppState>) -> Json<RegisterResponse> {
    Json(RegisterResponse {
        token: state.register_token().await,
    })
}

async fn put_pixel(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<PixelRequest>,
) -> Result<Json<PixelResponse>, ApiError> {
    let token = headers
        .get("x-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    state.authorize(token).await?;
    state.set_pixel(req.x, req.y, req.color).await?;
    Ok(Json(PixelResponse { ok: true }))
}

/// Decrements the live-viewer count when its SSE connection is dropped.
struct OnlineGuard {
    state: AppState,
}

impl Drop for OnlineGuard {
    fn drop(&mut self) {
        let state = self.state.clone();
        tokio::spawn(async move { state.online_add(-1).await });
    }
}

async fn sse_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    state.online_add(1).await;
    // Tie a guard to the stream so the count drops when the client disconnects.
    let guard = OnlineGuard {
        state: state.clone(),
    };
    let stream = state.events().map(move |ev| {
        let _ = &guard; // keep the guard alive for the stream's lifetime
        ev
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Build the application router.
pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/info", get(info))
        .route("/api/canvas", get(get_canvas))
        .route("/register", post(register))
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

    #[test]
    fn drain_dedups_and_formats() {
        // Empty buffer -> nothing to broadcast.
        assert_eq!(drain_to_batch(HashMap::new()), None);

        // One pixel -> a single-element JSON array.
        let mut one = HashMap::new();
        one.insert((5u16, 6u16), 9u8);
        assert_eq!(
            drain_to_batch(one),
            Some(r#"[{"x":5,"y":6,"color":9}]"#.to_string())
        );

        // Same cell repainted in the window -> last write wins (one entry).
        let mut dup = HashMap::new();
        dup.insert((1u16, 2u16), 3u8);
        dup.insert((1u16, 2u16), 7u8);
        assert_eq!(
            drain_to_batch(dup),
            Some(r#"[{"x":1,"y":2,"color":7}]"#.to_string())
        );
    }

    #[tokio::test]
    async fn painting_emits_a_batched_live_event() {
        std::env::set_var("SSE_COALESCE_MS", "5");
        let state = AppState::new(None).await;
        let mut rx = state.subscribe();
        assert!(state.set_pixel(3, 4, 7).await.is_ok());
        // Updates are coalesced, so the event is a JSON array delivered a tick later.
        let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("a batch should be flushed within a tick")
            .unwrap();
        assert_eq!(msg, r#"[{"x":3,"y":4,"color":7}]"#);
    }

    #[tokio::test]
    async fn token_is_required_and_rate_limited() {
        let state = AppState::new(None).await;
        // No token / unknown token are rejected.
        assert!(matches!(
            state.authorize("").await,
            Err(ApiError::Unauthorized)
        ));
        assert!(matches!(
            state.authorize("forged").await,
            Err(ApiError::Unauthorized)
        ));
        // A known token is accepted up to the budget, then rate-limited.
        // (store_token skips the deliberate 5s registration delay.)
        state.store_token("t").await;
        for _ in 0..RATE_LIMIT {
            assert!(state.authorize("t").await.is_ok());
        }
        assert!(matches!(
            state.authorize("t").await,
            Err(ApiError::RateLimited)
        ));
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
        assert!(body.contains(r#""width":200"#), "got: {body}");
        assert!(body.contains(r#""height":200"#), "got: {body}");
    }
}
