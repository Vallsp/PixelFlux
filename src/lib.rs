//! Collaborative pixel canvas — core application logic.
//!
//! A shared `WIDTH x HEIGHT` grid where each cell holds an RGB colour encoded
//! as 6 hex characters (`rrggbb`). The whole canvas is a single
//! `WIDTH*HEIGHT*6` string.
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
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

/// Default canvas dimensions. The live dimensions are held in `Settings`
/// (admin-editable); these are the starting values for a fresh deployment.
pub const WIDTH: usize = 200;
pub const HEIGHT: usize = 200;
/// Each pixel is stored as a 6-hex-digit RGB colour (`rrggbb`), so the canvas
/// supports the full 16M-colour space rather than a fixed palette.
const BYTES_PER_PIXEL: usize = 6;
// Versioned: the RGB format (6 hex/pixel) is incompatible with the old palette
// canvas (1 hex/pixel), so use a fresh key rather than corrupt the old one.
const CANVAS_KEY: &str = "canvas:rgb";
const EVENTS_CHANNEL: &str = "canvas:events";
/// Redis key holding the live, admin-tunable settings (JSON).
const CONFIG_KEY: &str = "config";
/// Pub/sub channel: a message here tells every instance to reload `CONFIG_KEY`,
/// so an admin change propagates across all replicas (same pattern as pixels).
const CONFIG_CHANNEL: &str = "config:events";
/// Redis counters for the admin dashboard.
const STATS_PIXELS_KEY: &str = "stats:pixels";
const STATS_TOKENS_KEY: &str = "stats:tokens";
/// Prefix for server-side admin session tokens (Redis: `admin:session:{id}`).
const ADMIN_SESSION_PREFIX: &str = "admin:session:";
/// Admin session lifetime.
const ADMIN_SESSION_TTL_SECS: u64 = 3_600;
/// Sorted-set key in Redis for per-player pixel counts.
const LEADERBOARD_KEY: &str = "leaderboard:scores";
/// Prefix for the pseudo→token reservation keys (`name:{normalised}` = token),
/// which make each pseudo unique and bound to one token.
const NAME_KEY_PREFIX: &str = "name:";

// Defaults for the runtime-tunable `Settings`. The live values are held in
// `AppState` (and Redis); these are only the starting point.

/// Registration is deliberately slow so minting tokens en masse is expensive
/// (you can't just create a fresh user on every paint request).
const DEFAULT_REGISTER_DELAY_SECS: u64 = 5;
/// How long an issued token stays valid server-side (clients re-register on 401).
const DEFAULT_TOKEN_TTL_SECS: u64 = 86_400;
/// Per-token paint budget within a rolling window.
const DEFAULT_RATE_LIMIT: u64 = 4096;
const DEFAULT_RATE_WINDOW_SECS: u64 = 30;

/// Live-viewer tracking: each connected tab refreshes its entry every
/// `online_heartbeat_secs`; entries older than `online_ttl_secs` are pruned, so
/// dead pods and dropped tabs self-heal instead of leaking the count.
const DEFAULT_ONLINE_HEARTBEAT_SECS: u64 = 5;
const DEFAULT_ONLINE_TTL_SECS: i64 = 15;

/// Smallest and largest editable canvas side (pixels). A 512² canvas is ~1.5 MB
/// as a hex string — fine for Redis and the full-fetch — while staying sane.
const MIN_CANVAS_SIDE: usize = 8;
const MAX_CANVAS_SIDE: usize = 512;

fn default_canvas_width() -> usize {
    WIDTH
}
fn default_canvas_height() -> usize {
    HEIGHT
}
fn default_true() -> bool {
    true
}
fn default_sse_coalesce_ms() -> u64 {
    16
}
fn default_palette() -> Vec<String> {
    PALETTE.iter().map(|s| s.to_string()).collect()
}
fn default_maintenance_message() -> String {
    "Maintenance en cours — le canevas est en lecture seule, la peinture est \
     temporairement désactivée."
        .to_string()
}
/// Char-safe truncation so an over-long admin message can't bloat `/info`.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Runtime-tunable settings, editable from the admin page and shared across
/// replicas via Redis. Each value mirrors a former compile-time constant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Canvas width in pixels. Changing it resets the canvas to blank.
    #[serde(default = "default_canvas_width")]
    pub width: usize,
    /// Canvas height in pixels. Changing it resets the canvas to blank.
    #[serde(default = "default_canvas_height")]
    pub height: usize,
    /// Master switch for the per-token paint budget. When false, painting is
    /// unlimited (token still required, maintenance still applies).
    #[serde(default = "default_true")]
    pub rate_limit_enabled: bool,
    /// Max pixels a token may paint per `rate_window_secs`.
    pub rate_limit: u64,
    /// Length of the rolling rate-limit window, in seconds.
    pub rate_window_secs: u64,
    /// Artificial delay applied when issuing a token, in seconds (anti-abuse).
    pub register_delay_secs: u64,
    /// How long an issued token stays valid, in seconds.
    pub token_ttl_secs: u64,
    /// Viewer heartbeat interval, in seconds.
    pub online_heartbeat_secs: u64,
    /// Viewer entry expiry, in seconds.
    pub online_ttl_secs: i64,
    /// SSE coalescing window in milliseconds: pixel updates are buffered and
    /// flushed once per window, trading a little latency for far less fan-out
    /// (cost becomes `ticks × clients` instead of `writes × clients`).
    #[serde(default = "default_sse_coalesce_ms")]
    pub sse_coalesce_ms: u64,
    /// Preset colours offered as quick swatches in the UI (`#rrggbb`).
    #[serde(default = "default_palette")]
    pub palette: Vec<String>,
    /// Whether the public UI offers the native colour picker (the "pipette").
    /// When false, visitors are limited to the preset palette.
    #[serde(default = "default_true")]
    pub color_picker_enabled: bool,
    /// Server-side enforcement: when true, `set_pixel` rejects any colour that
    /// is not in `palette` (so the API can't be used to bypass the UI).
    #[serde(default)]
    pub enforce_palette: bool,
    /// Whether new visitors may register a paint token. When false, `/register`
    /// returns 503 (existing painters keep working).
    #[serde(default = "default_true")]
    pub registration_open: bool,
    /// Banner text shown to visitors while `paused` is true.
    #[serde(default = "default_maintenance_message")]
    pub maintenance_message: String,
    /// Free-text announcement shown to everyone (empty = no banner).
    #[serde(default)]
    pub announcement: String,
    /// Maintenance switch: when true the canvas is read-only (paint → 503).
    pub paused: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            width: default_canvas_width(),
            height: default_canvas_height(),
            rate_limit_enabled: true,
            rate_limit: DEFAULT_RATE_LIMIT,
            rate_window_secs: DEFAULT_RATE_WINDOW_SECS,
            register_delay_secs: DEFAULT_REGISTER_DELAY_SECS,
            token_ttl_secs: DEFAULT_TOKEN_TTL_SECS,
            online_heartbeat_secs: DEFAULT_ONLINE_HEARTBEAT_SECS,
            online_ttl_secs: DEFAULT_ONLINE_TTL_SECS,
            sse_coalesce_ms: default_sse_coalesce_ms(),
            palette: default_palette(),
            color_picker_enabled: true,
            enforce_palette: false,
            registration_open: true,
            maintenance_message: default_maintenance_message(),
            announcement: String::new(),
            paused: false,
        }
    }
}

impl Settings {
    /// Clamp incoming values to sane bounds so the admin can't brick the app
    /// (e.g. a zero window that divides by nothing, or absurd delays/sizes).
    fn sanitized(mut self) -> Self {
        self.width = self.width.clamp(MIN_CANVAS_SIDE, MAX_CANVAS_SIDE);
        self.height = self.height.clamp(MIN_CANVAS_SIDE, MAX_CANVAS_SIDE);
        self.rate_limit = self.rate_limit.clamp(1, 1_000_000);
        self.rate_window_secs = self.rate_window_secs.clamp(1, 3_600);
        self.register_delay_secs = self.register_delay_secs.min(60);
        self.token_ttl_secs = self.token_ttl_secs.clamp(60, 2_592_000);
        self.online_heartbeat_secs = self.online_heartbeat_secs.clamp(1, 300);
        self.online_ttl_secs = self.online_ttl_secs.clamp(2, 600);
        self.sse_coalesce_ms = self.sse_coalesce_ms.clamp(1, 1_000);
        // Palette: keep only valid 6-hex colours, normalised to `#rrggbb`, deduped
        // and capped. Fall back to the default set if nothing valid remains.
        let mut palette: Vec<String> = Vec::new();
        for c in &self.palette {
            if let Some(hex) = normalize_color(c) {
                let entry = format!("#{hex}");
                if !palette.contains(&entry) {
                    palette.push(entry);
                }
            }
            if palette.len() >= 64 {
                break;
            }
        }
        self.palette = if palette.is_empty() {
            default_palette()
        } else {
            palette
        };
        self.maintenance_message = truncate_chars(self.maintenance_message.trim(), 200);
        if self.maintenance_message.is_empty() {
            self.maintenance_message = default_maintenance_message();
        }
        self.announcement = truncate_chars(self.announcement.trim(), 200);
        self
    }
}

/// Live counters surfaced on the admin dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub pixels_painted: u64,
    pub tokens_issued: u64,
    pub online: i64,
}

/// Default preset colours offered as quick swatches in the UI. Pixels can be
/// any RGB colour; these are just convenient starting points.
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
    #[error("color {0:?} is not a 6-digit hex colour (rrggbb)")]
    InvalidColor(String),
    #[error("color {0:?} is not in the allowed palette")]
    NotInPalette(String),
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
    #[error("rate limit exceeded — too many pixels in the current window")]
    RateLimited,
    #[error("canvas is in maintenance mode (read-only)")]
    Paused,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            ApiError::Pixel(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ApiError::Paused => StatusCode::SERVICE_UNAVAILABLE,
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

/// Why a registration was refused.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("pseudo is empty or invalid")]
    InvalidName,
    #[error("pseudo is already taken")]
    NameTaken,
}

/// Keep only alphanumeric characters, spaces, underscores, and hyphens;
/// trim whitespace; cap at 20 code-points.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'))
        .take(20)
        .collect::<String>()
        .trim()
        .to_string()
}

fn blank_canvas(width: usize, height: usize) -> Vec<u8> {
    // White background: "ffffff" per pixel.
    b"ffffff".repeat(width * height)
}

/// Validate and normalise a colour to 6 lowercase hex digits (no `#`), or
/// `None` if it isn't a valid RGB colour.
fn normalize_color(color: &str) -> Option<String> {
    let c = color.trim().trim_start_matches('#');
    if c.len() == BYTES_PER_PIXEL && c.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(c.to_ascii_lowercase())
    } else {
        None
    }
}

/// Current Unix time in seconds (used as the heartbeat score for viewers).
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Length-aware constant-time byte comparison, to avoid leaking the admin
/// password via timing on the login path.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Pull the `admin_session` value out of the request's `Cookie` header.
fn admin_session_cookie(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| {
            raw.split(';').find_map(|c| {
                let (k, v) = c.trim().split_once('=')?;
                (k == "admin_session").then(|| v.to_string())
            })
        })
        .unwrap_or_default()
}

/// Pending pixel updates, keyed by `(x, y)` so a cell repainted within the same
/// window keeps only its last colour. Flushed to SSE clients on a fixed tick.
type PixelBuffer = Arc<Mutex<HashMap<(u16, u16), String>>>;

/// One pixel update — the unit of the Redis pub/sub payload and of each entry
/// in a batched SSE event. `color` is a 6-hex RGB string.
#[derive(Serialize, Deserialize)]
struct PixelEvent {
    x: u16,
    y: u16,
    color: String,
}

/// Drain a buffer into one batched SSE payload: a JSON array of pixel updates,
/// or `None` if empty. Pure (no timing or I/O) so it is unit-testable directly.
fn drain_to_batch(map: HashMap<(u16, u16), String>) -> Option<String> {
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
fn spawn_flusher(
    buffer: PixelBuffer,
    tx: broadcast::Sender<String>,
    settings: Arc<Mutex<Settings>>,
) {
    tokio::spawn(async move {
        loop {
            // Read the live coalescing window each tick so an admin change to
            // `sse_coalesce_ms` takes effect without a restart.
            let period_ms = settings.lock().unwrap().sse_coalesce_ms.max(1);
            tokio::time::sleep(Duration::from_millis(period_ms)).await;
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

/// Background task: when an admin changes the settings, a message is published
/// on `CONFIG_CHANNEL`; every instance listens and reloads `CONFIG_KEY` into its
/// in-memory `settings` so the change takes effect fleet-wide. Reconnects on
/// failure.
fn spawn_config_subscriber(
    client: redis::Client,
    conn: redis::aio::ConnectionManager,
    settings: Arc<Mutex<Settings>>,
) {
    tokio::spawn(async move {
        loop {
            if let Ok(mut pubsub) = client.get_async_pubsub().await {
                if pubsub.subscribe(CONFIG_CHANNEL).await.is_ok() {
                    let mut stream = pubsub.on_message();
                    while stream.next().await.is_some() {
                        let mut c = conn.clone();
                        if let Ok(Some(json)) = redis::cmd("GET")
                            .arg(CONFIG_KEY)
                            .query_async::<Option<String>>(&mut c)
                            .await
                        {
                            if let Ok(s) = serde_json::from_str::<Settings>(&json) {
                                *settings.lock().unwrap() = s.sanitized();
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    redis: Option<redis::aio::ConnectionManager>,
    fallback: Arc<Mutex<Vec<u8>>>,
    fallback_lb: Arc<Mutex<HashMap<String, u64>>>,
    tx: broadcast::Sender<String>,
    /// Pending pixel updates, flushed to SSE clients on a tick (coalesced).
    buffer: PixelBuffer,
    /// Issued tokens → bound player name (used only when Redis is unavailable).
    tokens: Arc<Mutex<HashMap<String, String>>>,
    /// Reserved pseudos (normalised) → token, for uniqueness (fallback, no Redis).
    names: Arc<Mutex<HashMap<String, String>>>,
    /// Per-token paint counts + window start (fallback rate limiter).
    rates: Arc<Mutex<HashMap<String, (u64, Instant)>>>,
    /// Live viewers by connection id → last heartbeat (fallback when no Redis).
    online: Arc<Mutex<HashMap<String, Instant>>>,
    /// Runtime-tunable settings (admin-editable, shared across replicas).
    settings: Arc<Mutex<Settings>>,
    /// Cumulative counters (fallback when no Redis).
    pixels_painted: Arc<Mutex<u64>>,
    tokens_issued: Arc<Mutex<u64>>,
    /// Active admin sessions → issued-at (fallback when no Redis).
    admin_sessions: Arc<Mutex<HashMap<String, Instant>>>,
    /// Admin password from `ADMIN_PASSWORD`; `None` disables the admin entirely.
    admin_password: Option<Arc<String>>,
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

        let settings = Arc::new(Mutex::new(Settings::default()));
        // SSE_COALESCE_MS seeds the starting coalescing window (tests lower it to
        // flush almost immediately); the admin can change it live afterwards.
        if let Some(ms) = std::env::var("SSE_COALESCE_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
        {
            settings.lock().unwrap().sse_coalesce_ms = ms.max(1);
        }
        // Coalesce the SSE fan-out onto a tick (default 16ms). The flusher reads
        // the live setting each tick, so admin changes apply without a restart.
        spawn_flusher(buffer.clone(), tx.clone(), settings.clone());

        let mut redis = None;
        if let Some(url) = redis_url {
            if let Ok(client) = redis::Client::open(url) {
                if let Ok(conn) = redis::aio::ConnectionManager::new(client.clone()).await {
                    let mut init = conn.clone();
                    // Resolve settings first — a peer may have changed the canvas
                    // dimensions — then seed the canvas at the resolved size.
                    match redis::cmd("GET")
                        .arg(CONFIG_KEY)
                        .query_async::<Option<String>>(&mut init)
                        .await
                    {
                        Ok(Some(json)) => {
                            if let Ok(s) = serde_json::from_str::<Settings>(&json) {
                                *settings.lock().unwrap() = s.sanitized();
                            }
                        }
                        _ => {
                            // Serialise into a String first so the MutexGuard is
                            // dropped before we await the Redis write.
                            let json = serde_json::to_string(&*settings.lock().unwrap()).ok();
                            if let Some(json) = json {
                                let _ = redis::cmd("SET")
                                    .arg(CONFIG_KEY)
                                    .arg(json)
                                    .query_async::<String>(&mut init)
                                    .await;
                            }
                        }
                    }
                    let (w, h) = {
                        let s = settings.lock().unwrap();
                        (s.width, s.height)
                    };
                    let blank = String::from_utf8(blank_canvas(w, h)).unwrap();
                    let _ = redis::cmd("SETNX")
                        .arg(CANVAS_KEY)
                        .arg(blank)
                        .query_async::<i64>(&mut init)
                        .await;
                    spawn_event_subscriber(client.clone(), buffer.clone());
                    spawn_config_subscriber(client, conn.clone(), settings.clone());
                    redis = Some(conn);
                }
            }
        }

        let admin_password = std::env::var("ADMIN_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty())
            .map(Arc::new);

        let (fw, fh) = {
            let s = settings.lock().unwrap();
            (s.width, s.height)
        };

        Self {
            redis,
            fallback: Arc::new(Mutex::new(blank_canvas(fw, fh))),
            fallback_lb: Arc::new(Mutex::new(HashMap::new())),
            tx,
            buffer,
            tokens: Arc::new(Mutex::new(HashMap::new())),
            names: Arc::new(Mutex::new(HashMap::new())),
            rates: Arc::new(Mutex::new(HashMap::new())),
            online: Arc::new(Mutex::new(HashMap::new())),
            settings,
            pixels_painted: Arc::new(Mutex::new(0)),
            tokens_issued: Arc::new(Mutex::new(0)),
            admin_sessions: Arc::new(Mutex::new(HashMap::new())),
            admin_password,
        }
    }

    /// A snapshot of the current runtime settings.
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Return the whole canvas as a `width*height` hex string.
    pub async fn canvas(&self) -> String {
        let (width, height) = {
            let s = self.settings.lock().unwrap();
            (s.width, s.height)
        };
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            if let Ok(Some(s)) = redis::cmd("GET")
                .arg(CANVAS_KEY)
                .query_async::<Option<String>>(&mut conn)
                .await
            {
                if s.len() == width * height * BYTES_PER_PIXEL {
                    return s;
                }
            }
        }
        String::from_utf8(self.fallback.lock().unwrap().clone()).unwrap()
    }

    /// Increment the pixel count for `name` in the leaderboard sorted set.
    async fn increment_score(&self, name: &str) {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("ZINCRBY")
                .arg(LEADERBOARD_KEY)
                .arg(1i64)
                .arg(name)
                .query_async::<f64>(&mut conn)
                .await;
        } else {
            let mut lb = self.fallback_lb.lock().unwrap();
            *lb.entry(name.to_string()).or_insert(0) += 1;
        }
    }

    /// Return the top-10 players sorted by pixel count (highest first).
    pub async fn leaderboard(&self) -> Vec<LeaderboardEntry> {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            if let Ok(pairs) = redis::cmd("ZREVRANGE")
                .arg(LEADERBOARD_KEY)
                .arg(0i64)
                .arg(9i64)
                .arg("WITHSCORES")
                .query_async::<Vec<String>>(&mut conn)
                .await
            {
                return pairs
                    .chunks(2)
                    .filter_map(|chunk| match chunk {
                        [name, score] => Some(LeaderboardEntry {
                            name: name.clone(),
                            count: score.parse::<f64>().unwrap_or(0.0) as u64,
                        }),
                        _ => None,
                    })
                    .collect();
            }
        }
        let lb = self.fallback_lb.lock().unwrap();
        let mut entries: Vec<LeaderboardEntry> = lb
            .iter()
            .map(|(name, &count)| LeaderboardEntry {
                name: name.clone(),
                count,
            })
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.count));
        entries.truncate(10);
        entries
    }

    /// Paint a single pixel. `player` (if provided and non-empty after
    /// sanitisation) is credited in the leaderboard. Returns an error if the
    /// coordinates or colour are invalid. With Redis the update is published to
    /// every instance; the leaderboard snapshot is broadcast in-process over SSE.
    pub async fn set_pixel(
        &self,
        x: usize,
        y: usize,
        color: &str,
        player: Option<&str>,
    ) -> Result<(), PixelError> {
        let normalized =
            normalize_color(color).ok_or_else(|| PixelError::InvalidColor(color.to_string()))?;
        // Read dimensions and (if enforced) check palette membership in one lock,
        // without cloning the palette on this hot path.
        let (width, height, rejected) = {
            let s = self.settings.lock().unwrap();
            let rejected = s.enforce_palette
                && !s
                    .palette
                    .iter()
                    .any(|c| c.trim_start_matches('#') == normalized.as_str());
            (s.width, s.height, rejected)
        };
        if rejected {
            return Err(PixelError::NotInPalette(normalized));
        }
        if x >= width || y >= height {
            return Err(PixelError::OutOfBounds {
                x,
                y,
                width,
                height,
            });
        }
        let color = normalized;
        let offset = (y * width + x) * BYTES_PER_PIXEL;
        let payload = format!(r#"{{"x":{x},"y":{y},"color":"{color}"}}"#);

        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let wrote = redis::cmd("SETRANGE")
                .arg(CANVAS_KEY)
                .arg(offset)
                .arg(&color)
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
                let _ = redis::cmd("INCR")
                    .arg(STATS_PIXELS_KEY)
                    .query_async::<i64>(&mut conn)
                    .await;
            } else {
                let mut fb = self.fallback.lock().unwrap();
                fb[offset..offset + BYTES_PER_PIXEL].copy_from_slice(color.as_bytes());
                // Feed the coalescing buffer; the flusher batches and broadcasts it.
                self.buffer
                    .lock()
                    .unwrap()
                    .insert((x as u16, y as u16), color.clone());
                *self.pixels_painted.lock().unwrap() += 1;
            }
        } else {
            let mut fb = self.fallback.lock().unwrap();
            fb[offset..offset + BYTES_PER_PIXEL].copy_from_slice(color.as_bytes());
            self.buffer
                .lock()
                .unwrap()
                .insert((x as u16, y as u16), color.clone());
            *self.pixels_painted.lock().unwrap() += 1;
        }

        // Update leaderboard and push fresh top-10 to connected SSE clients.
        if let Some(name) = player {
            let safe = sanitize_name(name);
            if !safe.is_empty() {
                self.increment_score(&safe).await;
                let lb = self.leaderboard().await;
                if let Ok(lb_json) = serde_json::to_string(&lb) {
                    let _ = self.tx.send(format!("lb:{lb_json}"));
                }
            }
        }

        Ok(())
    }

    /// Persist a bare token bound to no pseudo (used by tests to mint a token
    /// without going through the slow, name-reserving registration).
    #[cfg(test)]
    async fn store_token(&self, token: &str) {
        let ttl = self.settings().token_ttl_secs;
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("SET")
                .arg(format!("token:{token}"))
                .arg("")
                .arg("EX")
                .arg(ttl)
                .query_async::<String>(&mut conn)
                .await;
            let _ = redis::cmd("INCR")
                .arg(STATS_TOKENS_KEY)
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            self.tokens
                .lock()
                .unwrap()
                .insert(token.to_string(), String::new());
            *self.tokens_issued.lock().unwrap() += 1;
        }
    }

    /// Register a player: reserve a **unique** pseudo and mint a token bound to
    /// it. The binding lives server-side, so the pseudo can't be spoofed from a
    /// paint request. Deliberately slow (`register_delay_secs`) as anti-abuse.
    ///
    /// Returns `(token, sanitized_name)`, or an error if the pseudo is invalid
    /// or already taken.
    pub async fn register_player(&self, name: &str) -> Result<(String, String), RegisterError> {
        let safe = sanitize_name(name);
        if safe.is_empty() {
            return Err(RegisterError::InvalidName);
        }
        let key = safe.to_lowercase();
        let ttl = self.settings().token_ttl_secs;
        let delay = self.settings().register_delay_secs;

        // Fast pre-check so an obviously-taken name fails without the delay.
        if self.name_taken(&key).await {
            return Err(RegisterError::NameTaken);
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;

        let token = uuid::Uuid::new_v4().to_string();
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            // Atomically reserve the name; `NX` fails if a peer grabbed it first.
            let reserved: Option<String> = redis::cmd("SET")
                .arg(format!("{NAME_KEY_PREFIX}{key}"))
                .arg(&token)
                .arg("NX")
                .arg("EX")
                .arg(ttl)
                .query_async(&mut conn)
                .await
                .ok()
                .flatten();
            if reserved.is_none() {
                return Err(RegisterError::NameTaken);
            }
            let _ = redis::cmd("SET")
                .arg(format!("token:{token}"))
                .arg(&safe)
                .arg("EX")
                .arg(ttl)
                .query_async::<String>(&mut conn)
                .await;
            let _ = redis::cmd("INCR")
                .arg(STATS_TOKENS_KEY)
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            let mut names = self.names.lock().unwrap();
            if names.contains_key(&key) {
                return Err(RegisterError::NameTaken);
            }
            names.insert(key, token.clone());
            drop(names);
            self.tokens
                .lock()
                .unwrap()
                .insert(token.clone(), safe.clone());
            *self.tokens_issued.lock().unwrap() += 1;
        }
        Ok((token, safe))
    }

    /// Whether a normalised pseudo is currently reserved.
    async fn name_taken(&self, key: &str) -> bool {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            redis::cmd("EXISTS")
                .arg(format!("{NAME_KEY_PREFIX}{key}"))
                .query_async(&mut conn)
                .await
                .unwrap_or(false)
        } else {
            self.names.lock().unwrap().contains_key(key)
        }
    }

    /// The pseudo bound to `token`, if any (empty binding = none).
    pub async fn player_for_token(&self, token: &str) -> Option<String> {
        if token.is_empty() {
            return None;
        }
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let name: Option<String> = redis::cmd("GET")
                .arg(format!("token:{token}"))
                .query_async(&mut conn)
                .await
                .ok()
                .flatten();
            name.filter(|n| !n.is_empty())
        } else {
            self.tokens
                .lock()
                .unwrap()
                .get(token)
                .filter(|n| !n.is_empty())
                .cloned()
        }
    }

    /// Validate `token` and count this paint against its budget. Returns
    /// `Unauthorized` for an unknown token and `RateLimited` past the window
    /// limit. Backed by Redis (shared across replicas) or an in-process store.
    pub async fn authorize(&self, token: &str) -> Result<(), ApiError> {
        if token.is_empty() {
            return Err(ApiError::Unauthorized);
        }
        let cfg = self.settings();
        // Maintenance mode: reject paints fleet-wide, regardless of token.
        if cfg.paused {
            return Err(ApiError::Paused);
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
            if cfg.rate_limit_enabled {
                let key = format!("rate:{token}");
                let count: i64 = redis::cmd("INCR")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await
                    .unwrap_or(1);
                if count == 1 {
                    let _ = redis::cmd("EXPIRE")
                        .arg(&key)
                        .arg(cfg.rate_window_secs)
                        .query_async::<i64>(&mut conn)
                        .await;
                }
                if count as u64 > cfg.rate_limit {
                    return Err(ApiError::RateLimited);
                }
            }
        } else {
            if !self.tokens.lock().unwrap().contains_key(token) {
                return Err(ApiError::Unauthorized);
            }
            if cfg.rate_limit_enabled {
                let mut rates = self.rates.lock().unwrap();
                let now = Instant::now();
                let entry = rates.entry(token.to_string()).or_insert((0, now));
                if now.duration_since(entry.1).as_secs() >= cfg.rate_window_secs {
                    *entry = (0, now);
                }
                entry.0 += 1;
                if entry.0 > cfg.rate_limit {
                    return Err(ApiError::RateLimited);
                }
            }
        }
        Ok(())
    }

    /// Mark a viewer (by connection id) as seen now. Called on connect and on
    /// each heartbeat. In Redis this is a sorted set scored by timestamp, shared
    /// across replicas; otherwise an in-process map.
    async fn online_seen(&self, cid: &str) {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("ZADD")
                .arg("viewers")
                .arg(now_secs())
                .arg(cid)
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            self.online
                .lock()
                .unwrap()
                .insert(cid.to_string(), Instant::now());
        }
    }

    /// Remove a viewer immediately when its connection closes cleanly.
    async fn online_gone(&self, cid: &str) {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("ZREM")
                .arg("viewers")
                .arg(cid)
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            self.online.lock().unwrap().remove(cid);
        }
    }

    /// Current number of connected viewers. Prunes entries whose heartbeat went
    /// stale first, so dropped tabs and dead pods don't inflate the count.
    pub async fn online(&self) -> i64 {
        let ttl_secs = self.settings().online_ttl_secs;
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let cutoff = now_secs() - ttl_secs;
            let _ = redis::cmd("ZREMRANGEBYSCORE")
                .arg("viewers")
                .arg("-inf")
                .arg(cutoff)
                .query_async::<i64>(&mut conn)
                .await;
            redis::cmd("ZCARD")
                .arg("viewers")
                .query_async::<i64>(&mut conn)
                .await
                .unwrap_or(0)
        } else {
            let ttl = Duration::from_secs(ttl_secs.max(0) as u64);
            let mut map = self.online.lock().unwrap();
            map.retain(|_, seen| seen.elapsed() < ttl);
            map.len() as i64
        }
    }

    /// Apply new settings: clamp them, store in-memory, and (with Redis) persist
    /// to `CONFIG_KEY` and publish on `CONFIG_CHANNEL` so every replica reloads.
    pub async fn update_settings(&self, new: Settings) -> Settings {
        let clean = new.sanitized();
        let resized = {
            let mut g = self.settings.lock().unwrap();
            let changed = g.width != clean.width || g.height != clean.height;
            *g = clean.clone();
            changed
        };
        if let Some(conn) = &self.redis {
            if let Ok(json) = serde_json::to_string(&clean) {
                let mut conn = conn.clone();
                let _ = redis::cmd("SET")
                    .arg(CONFIG_KEY)
                    .arg(json)
                    .query_async::<String>(&mut conn)
                    .await;
                let _ = redis::cmd("PUBLISH")
                    .arg(CONFIG_CHANNEL)
                    .arg("changed")
                    .query_async::<i64>(&mut conn)
                    .await;
            }
        }
        // A different canvas size is incompatible with the stored pixels, so
        // reset the canvas to a fresh blank at the new size.
        if resized {
            self.clear_canvas().await;
        }
        clean
    }

    /// Live counters for the admin dashboard.
    pub async fn stats(&self) -> Stats {
        let online = self.online().await;
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let pixels: i64 = redis::cmd("GET")
                .arg(STATS_PIXELS_KEY)
                .query_async::<Option<i64>>(&mut conn)
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
            let tokens: i64 = redis::cmd("GET")
                .arg(STATS_TOKENS_KEY)
                .query_async::<Option<i64>>(&mut conn)
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
            Stats {
                pixels_painted: pixels.max(0) as u64,
                tokens_issued: tokens.max(0) as u64,
                online,
            }
        } else {
            Stats {
                pixels_painted: *self.pixels_painted.lock().unwrap(),
                tokens_issued: *self.tokens_issued.lock().unwrap(),
                online,
            }
        }
    }

    /// Wipe the canvas back to white and tell every connected client to clear,
    /// so the reset is immediate rather than waiting for the next full resync.
    pub async fn clear_canvas(&self) {
        let (width, height) = {
            let s = self.settings.lock().unwrap();
            (s.width, s.height)
        };
        let blank = String::from_utf8(blank_canvas(width, height)).unwrap();
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("SET")
                .arg(CANVAS_KEY)
                .arg(&blank)
                .query_async::<String>(&mut conn)
                .await;
        }
        *self.fallback.lock().unwrap() = blank.into_bytes();
        // Sentinel understood by the client as "wipe + resync at current size".
        let _ = self.tx.send(r#"{"clear":true}"#.to_string());
    }

    /// Remove a player (admin): revoke the token bound to their pseudo, free the
    /// pseudo reservation, and drop them from the leaderboard. The refreshed
    /// top-10 is pushed to every client.
    pub async fn delete_player(&self, name: &str) {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            return;
        }
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            // Revoke the token bound to this pseudo (if any) so they can't keep
            // painting under it.
            if let Ok(Some(token)) = redis::cmd("GET")
                .arg(format!("{NAME_KEY_PREFIX}{key}"))
                .query_async::<Option<String>>(&mut conn)
                .await
            {
                let _ = redis::cmd("DEL")
                    .arg(format!("token:{token}"))
                    .query_async::<i64>(&mut conn)
                    .await;
            }
            let _ = redis::cmd("DEL")
                .arg(format!("{NAME_KEY_PREFIX}{key}"))
                .query_async::<i64>(&mut conn)
                .await;
            let _ = redis::cmd("ZREM")
                .arg(LEADERBOARD_KEY)
                .arg(name)
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            let token = self.names.lock().unwrap().remove(&key);
            if let Some(token) = token {
                self.tokens.lock().unwrap().remove(&token);
            }
            self.fallback_lb.lock().unwrap().remove(name);
        }
        // Push the refreshed leaderboard to every connected client.
        let lb = self.leaderboard().await;
        if let Ok(json) = serde_json::to_string(&lb) {
            let _ = self.tx.send(format!("lb:{json}"));
        }
    }

    /// Whether an admin password is configured (admin is disabled otherwise).
    pub fn admin_enabled(&self) -> bool {
        self.admin_password.is_some()
    }

    /// Validate a password in constant time and, on success, mint and store an
    /// opaque session id. Returns the session id, or `None` if disabled/wrong.
    pub async fn admin_login(&self, password: &str) -> Option<String> {
        let expected = self.admin_password.as_ref()?;
        if !constant_time_eq(password.as_bytes(), expected.as_bytes()) {
            return None;
        }
        let sid = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("SET")
                .arg(format!("{ADMIN_SESSION_PREFIX}{sid}"))
                .arg(1)
                .arg("EX")
                .arg(ADMIN_SESSION_TTL_SECS)
                .query_async::<String>(&mut conn)
                .await;
        } else {
            self.admin_sessions
                .lock()
                .unwrap()
                .insert(sid.clone(), Instant::now());
        }
        Some(sid)
    }

    /// True if `sid` is a currently-valid admin session.
    pub async fn admin_check(&self, sid: &str) -> bool {
        if self.admin_password.is_none() || sid.is_empty() {
            return false;
        }
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            redis::cmd("EXISTS")
                .arg(format!("{ADMIN_SESSION_PREFIX}{sid}"))
                .query_async(&mut conn)
                .await
                .unwrap_or(false)
        } else {
            let ttl = Duration::from_secs(ADMIN_SESSION_TTL_SECS);
            let mut map = self.admin_sessions.lock().unwrap();
            map.retain(|_, t| t.elapsed() < ttl);
            map.contains_key(sid)
        }
    }

    /// Invalidate an admin session (logout).
    pub async fn admin_logout(&self, sid: &str) {
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            let _ = redis::cmd("DEL")
                .arg(format!("{ADMIN_SESSION_PREFIX}{sid}"))
                .query_async::<i64>(&mut conn)
                .await;
        } else {
            self.admin_sessions.lock().unwrap().remove(sid);
        }
    }

    /// Subscribe to the live pixel event stream (the same feed used by SSE).
    /// Each message is a coalesced batch: a JSON array
    /// `[{"x":..,"y":..,"color":..}, ...]`.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// A stream of live events for SSE subscribers.
    /// Pixel batches → default "message" SSE type (one JSON array per tick).
    /// Leaderboard snapshots → "leaderboard" named SSE event.
    fn events(&self) -> impl Stream<Item = Result<Event, Infallible>> {
        BroadcastStream::new(self.tx.subscribe()).filter_map(|msg| match msg {
            Ok(json) if json.starts_with("lb:") => {
                let data = json["lb:".len()..].to_string();
                Some(Ok(Event::default().event("leaderboard").data(data)))
            }
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
    /// True when an admin has put the canvas in read-only maintenance mode.
    paused: bool,
    /// Whether new visitors can currently register a token.
    registration_open: bool,
    /// Banner text to show while `paused` is true.
    maintenance_message: String,
    /// Site-wide announcement (empty = none).
    announcement: String,
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
    palette: Vec<String>,
    /// Whether the native colour picker is offered (admin-toggleable).
    picker: bool,
    /// Whether only palette colours are accepted (server-enforced).
    enforce: bool,
    pixels: String,
}

#[derive(Serialize)]
pub struct LeaderboardEntry {
    name: String,
    count: u64,
}

#[derive(Deserialize)]
struct PixelRequest {
    x: usize,
    y: usize,
    /// RGB colour as 6 hex digits (`rrggbb`), with or without a leading `#`.
    color: String,
}

/// Registration payload: the pseudo the visitor wants to claim.
#[derive(Deserialize)]
struct RegisterRequest {
    player: String,
}

#[derive(Serialize)]
struct PixelResponse {
    ok: bool,
}

#[derive(Serialize)]
struct RegisterResponse {
    token: String,
    /// The sanitised pseudo the server actually bound to the token.
    name: String,
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
    let cfg = state.settings();
    Json(Info {
        name: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
        instance: instance_id(),
        online: state.online().await,
        paused: cfg.paused,
        registration_open: cfg.registration_open,
        maintenance_message: cfg.maintenance_message,
        announcement: cfg.announcement,
    })
}

async fn get_canvas(State(state): State<AppState>) -> Json<CanvasResponse> {
    let cfg = state.settings();
    Json(CanvasResponse {
        width: cfg.width,
        height: cfg.height,
        picker: cfg.color_picker_enabled,
        enforce: cfg.enforce_palette,
        palette: cfg.palette,
        pixels: state.canvas().await,
    })
}

/// Register a player and issue a token bound to their pseudo. Slow on purpose
/// (anti-abuse). Returns 503 when registration is closed, 400 for an invalid
/// pseudo, and 409 when the pseudo is already taken.
async fn register(State(state): State<AppState>, Json(req): Json<RegisterRequest>) -> Response {
    if !state.settings().registration_open {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                ok: false,
                error: "registration is currently closed".into(),
            }),
        )
            .into_response();
    }
    match state.register_player(&req.player).await {
        Ok((token, name)) => Json(RegisterResponse { token, name }).into_response(),
        Err(RegisterError::InvalidName) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                ok: false,
                error: "invalid pseudo".into(),
            }),
        )
            .into_response(),
        Err(RegisterError::NameTaken) => (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                ok: false,
                error: "pseudo already taken".into(),
            }),
        )
            .into_response(),
    }
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
    // The player name comes from the server-side token binding, never from the
    // request body — so a client can't paint under someone else's pseudo.
    let player = state.player_for_token(token).await;
    state
        .set_pixel(req.x, req.y, &req.color, player.as_deref())
        .await?;
    Ok(Json(PixelResponse { ok: true }))
}

/// Stops the heartbeat and removes the viewer when its SSE connection is dropped.
struct OnlineGuard {
    state: AppState,
    cid: String,
    heartbeat: tokio::task::JoinHandle<()>,
}

impl Drop for OnlineGuard {
    fn drop(&mut self) {
        self.heartbeat.abort();
        let state = self.state.clone();
        let cid = self.cid.clone();
        tokio::spawn(async move { state.online_gone(&cid).await });
    }
}

/// Optional stable connection id (one per browser tab) so reconnects of the same
/// tab don't count as new viewers.
#[derive(Deserialize)]
struct EventsParams {
    cid: Option<String>,
}

async fn sse_events(
    State(state): State<AppState>,
    Query(params): Query<EventsParams>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let cid = params
        .cid
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    state.online_seen(&cid).await;

    // Refresh the heartbeat while connected; the guard stops it and removes the
    // viewer when the stream is dropped.
    let hb_state = state.clone();
    let hb_cid = cid.clone();
    let hb_secs = state.settings().online_heartbeat_secs.max(1);
    let heartbeat = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(hb_secs));
        loop {
            tick.tick().await;
            hb_state.online_seen(&hb_cid).await;
        }
    });

    let guard = OnlineGuard {
        state: state.clone(),
        cid,
        heartbeat,
    };
    let stream = state.events().map(move |ev| {
        let _ = &guard; // keep the guard alive for the stream's lifetime
        ev
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
struct OkResponse {
    ok: bool,
}

/// Everything the admin dashboard needs in one round-trip.
#[derive(Serialize)]
struct AdminState {
    settings: Settings,
    stats: Stats,
    version: &'static str,
    instance: String,
    width: usize,
    height: usize,
}

async fn admin_page() -> Html<&'static str> {
    Html(include_str!("admin.html"))
}

/// Reject the request unless it carries a valid admin session cookie.
async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let sid = admin_session_cookie(headers);
    if state.admin_check(&sid).await {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                ok: false,
                error: "unauthorized".into(),
            }),
        )
            .into_response())
    }
}

async fn admin_login(State(state): State<AppState>, Json(req): Json<LoginRequest>) -> Response {
    match state.admin_login(&req.password).await {
        Some(sid) => {
            let cookie = format!(
                "admin_session={sid}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age={ADMIN_SESSION_TTL_SECS}"
            );
            let mut resp = Json(OkResponse { ok: true }).into_response();
            if let Ok(v) = axum::http::HeaderValue::from_str(&cookie) {
                resp.headers_mut().insert(axum::http::header::SET_COOKIE, v);
            }
            resp
        }
        None => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                ok: false,
                error: "invalid password".into(),
            }),
        )
            .into_response(),
    }
}

async fn admin_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let sid = admin_session_cookie(&headers);
    state.admin_logout(&sid).await;
    let clear = "admin_session=; HttpOnly; SameSite=Strict; Path=/admin; Max-Age=0";
    let mut resp = Json(OkResponse { ok: true }).into_response();
    resp.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_static(clear),
    );
    resp
}

async fn admin_get_state(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    let cfg = state.settings();
    Json(AdminState {
        width: cfg.width,
        height: cfg.height,
        settings: cfg,
        stats: state.stats().await,
        version: env!("CARGO_PKG_VERSION"),
        instance: instance_id(),
    })
    .into_response()
}

async fn admin_update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(new): Json<Settings>,
) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    let applied = state.update_settings(new).await;
    Json(applied).into_response()
}

async fn admin_clear_canvas(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    state.clear_canvas().await;
    Json(OkResponse { ok: true }).into_response()
}

/// A player to remove, identified by pseudo.
#[derive(Deserialize)]
struct DeletePlayerRequest {
    name: String,
}

async fn admin_delete_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DeletePlayerRequest>,
) -> Response {
    if let Err(e) = require_admin(&state, &headers).await {
        return e;
    }
    state.delete_player(&req.name).await;
    Json(OkResponse { ok: true }).into_response()
}

async fn get_leaderboard(State(state): State<AppState>) -> Json<Vec<LeaderboardEntry>> {
    Json(state.leaderboard().await)
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
        .route("/api/leaderboard", get(get_leaderboard))
        .route("/admin", get(admin_page))
        .route("/admin/login", post(admin_login))
        .route("/admin/logout", post(admin_logout))
        .route("/admin/api/state", get(admin_get_state))
        .route("/admin/api/settings", put(admin_update_settings))
        .route("/admin/api/canvas/clear", post(admin_clear_canvas))
        .route("/admin/api/players/delete", post(admin_delete_player))
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
        assert_eq!(canvas.len(), WIDTH * HEIGHT * BYTES_PER_PIXEL);
        assert!(canvas.chars().all(|c| c == 'f')); // white "ffffff" per pixel
    }

    #[tokio::test]
    async fn set_pixel_updates_the_canvas() {
        let state = AppState::new(None).await;
        assert!(state.set_pixel(1, 0, "ff0000", None).await.is_ok()); // pixel 1 -> red
        assert!(state.set_pixel(0, 1, "00ff00", None).await.is_ok()); // row below -> green

        let canvas = state.canvas().await;
        assert_eq!(&canvas[BYTES_PER_PIXEL..BYTES_PER_PIXEL * 2], "ff0000");
        let row = WIDTH * BYTES_PER_PIXEL;
        assert_eq!(&canvas[row..row + BYTES_PER_PIXEL], "00ff00");
    }

    #[tokio::test]
    async fn set_pixel_normalises_and_rejects() {
        let state = AppState::new(None).await;
        // A leading '#' and uppercase are accepted and normalised.
        assert!(state.set_pixel(0, 0, "#AABBCC", None).await.is_ok());
        assert_eq!(&state.canvas().await[0..BYTES_PER_PIXEL], "aabbcc");
        // Invalid inputs are rejected.
        assert!(state.set_pixel(WIDTH, 0, "ffffff", None).await.is_err()); // x out of bounds
        assert!(state.set_pixel(0, HEIGHT, "ffffff", None).await.is_err()); // y out of bounds
        assert!(state.set_pixel(0, 0, "12345", None).await.is_err()); // wrong length
        assert!(state.set_pixel(0, 0, "gggggg", None).await.is_err()); // non-hex
    }

    #[test]
    fn drain_dedups_and_formats() {
        // Empty buffer -> nothing to broadcast.
        assert_eq!(drain_to_batch(HashMap::new()), None);

        // One pixel -> a single-element JSON array.
        let mut one = HashMap::new();
        one.insert((5u16, 6u16), "abcdef".to_string());
        assert_eq!(
            drain_to_batch(one),
            Some(r#"[{"x":5,"y":6,"color":"abcdef"}]"#.to_string())
        );

        // Same cell repainted in the window -> last write wins (one entry).
        let mut dup = HashMap::new();
        dup.insert((1u16, 2u16), "111111".to_string());
        dup.insert((1u16, 2u16), "222222".to_string());
        assert_eq!(
            drain_to_batch(dup),
            Some(r#"[{"x":1,"y":2,"color":"222222"}]"#.to_string())
        );
    }

    #[tokio::test]
    async fn painting_emits_a_batched_live_event() {
        std::env::set_var("SSE_COALESCE_MS", "5");
        let state = AppState::new(None).await;
        let mut rx = state.subscribe();
        assert!(state.set_pixel(3, 4, "e50000", None).await.is_ok());
        // Updates are coalesced, so the event is a JSON array delivered a tick later.
        let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("a batch should be flushed within a tick")
            .unwrap();
        assert_eq!(msg, r#"[{"x":3,"y":4,"color":"e50000"}]"#);
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
        // (store_token skips the deliberate registration delay.)
        state.store_token("t").await;
        for _ in 0..DEFAULT_RATE_LIMIT {
            assert!(state.authorize("t").await.is_ok());
        }
        assert!(matches!(
            state.authorize("t").await,
            Err(ApiError::RateLimited)
        ));
    }

    #[tokio::test]
    async fn settings_update_changes_rate_limit() {
        let state = AppState::new(None).await;
        // Tighten the budget to 2 paints per window at runtime.
        let mut s = state.settings();
        s.rate_limit = 2;
        let applied = state.update_settings(s).await;
        assert_eq!(applied.rate_limit, 2);

        state.store_token("u").await;
        assert!(state.authorize("u").await.is_ok());
        assert!(state.authorize("u").await.is_ok());
        assert!(matches!(
            state.authorize("u").await,
            Err(ApiError::RateLimited)
        ));
    }

    #[tokio::test]
    async fn settings_are_clamped() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.rate_limit = 0; // invalid -> clamped to >= 1
        s.rate_window_secs = 0; // invalid -> clamped to >= 1
        let applied = state.update_settings(s).await;
        assert!(applied.rate_limit >= 1);
        assert!(applied.rate_window_secs >= 1);
    }

    #[tokio::test]
    async fn maintenance_mode_blocks_painting() {
        let state = AppState::new(None).await;
        state.store_token("m").await;
        assert!(state.authorize("m").await.is_ok());
        let mut s = state.settings();
        s.paused = true;
        state.update_settings(s).await;
        assert!(matches!(state.authorize("m").await, Err(ApiError::Paused)));
    }

    #[tokio::test]
    async fn rate_limit_can_be_disabled() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.rate_limit = 1;
        s.rate_limit_enabled = false;
        state.update_settings(s).await;
        state.store_token("r").await;
        // Far beyond a budget of 1 — all allowed because the limiter is off.
        for _ in 0..10 {
            assert!(state.authorize("r").await.is_ok());
        }
    }

    #[tokio::test]
    async fn resizing_resets_canvas_to_new_dimensions() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.width = 64;
        s.height = 32;
        state.update_settings(s).await;

        let canvas = state.canvas().await;
        assert_eq!(canvas.len(), 64 * 32 * BYTES_PER_PIXEL);
        assert!(canvas.chars().all(|c| c == 'f')); // fresh blank at the new size

        // The new bounds are enforced.
        assert!(state.set_pixel(63, 31, "ff0000", None).await.is_ok());
        assert!(state.set_pixel(64, 0, "ff0000", None).await.is_err());
    }

    #[tokio::test]
    async fn coalesce_window_is_tunable_and_clamped() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.sse_coalesce_ms = 50;
        assert_eq!(state.update_settings(s).await.sse_coalesce_ms, 50);
        // Out-of-range values are clamped (1..=1000).
        let mut s = state.settings();
        s.sse_coalesce_ms = 99_999;
        assert_eq!(state.update_settings(s).await.sse_coalesce_ms, 1_000);
    }

    #[tokio::test]
    async fn palette_keeps_only_valid_colours() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.palette = vec![
            "#ff0000".into(), // valid
            "00ff00".into(),  // valid, no '#'
            "nope".into(),    // invalid
            "#ff0000".into(), // duplicate
        ];
        let applied = state.update_settings(s).await;
        assert_eq!(
            applied.palette,
            vec!["#ff0000".to_string(), "#00ff00".to_string()]
        );

        // An all-invalid palette falls back to the default set.
        let mut s = state.settings();
        s.palette = vec!["bad".into(), "".into()];
        assert_eq!(state.update_settings(s).await.palette, default_palette());
    }

    #[tokio::test]
    async fn enforced_palette_rejects_other_colours() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.palette = vec!["#ff0000".into(), "#00ff00".into()];
        s.enforce_palette = true;
        state.update_settings(s).await;

        // Palette colours are accepted (case/`#` are normalised before the check).
        assert!(state.set_pixel(0, 0, "ff0000", None).await.is_ok());
        assert!(state.set_pixel(1, 0, "#00FF00", None).await.is_ok());
        // Anything else is rejected at the API level, not just hidden in the UI.
        assert!(matches!(
            state.set_pixel(2, 0, "0000ff", None).await,
            Err(PixelError::NotInPalette(_))
        ));

        // Turning enforcement off lets any valid colour through again.
        let mut s = state.settings();
        s.enforce_palette = false;
        state.update_settings(s).await;
        assert!(state.set_pixel(2, 0, "0000ff", None).await.is_ok());
    }

    #[tokio::test]
    async fn registration_can_be_closed() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.registration_open = false;
        state.update_settings(s).await;

        let response = app(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"player":"Alice"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn register_binds_a_unique_pseudo_to_the_token() {
        let state = AppState::new(None).await;
        // Skip the anti-abuse delay in the test.
        let mut s = state.settings();
        s.register_delay_secs = 0;
        state.update_settings(s).await;

        let (token, name) = state.register_player("Alice").await.expect("register");
        assert_eq!(name, "Alice");
        // The token resolves back to its bound pseudo (server-side, unspoofable).
        assert_eq!(
            state.player_for_token(&token).await.as_deref(),
            Some("Alice")
        );

        // Same pseudo (case-insensitive) is refused.
        assert!(matches!(
            state.register_player("alice").await,
            Err(RegisterError::NameTaken)
        ));
        // Empty/invalid pseudo is refused.
        assert!(matches!(
            state.register_player("   ").await,
            Err(RegisterError::InvalidName)
        ));
    }

    #[tokio::test]
    async fn unknown_token_has_no_pseudo() {
        let state = AppState::new(None).await;
        assert_eq!(state.player_for_token("nope").await, None);
        assert_eq!(state.player_for_token("").await, None);
    }

    #[tokio::test]
    async fn admin_can_delete_a_player() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.register_delay_secs = 0;
        state.update_settings(s).await;

        let (token, _name) = state.register_player("Bob").await.unwrap();
        state.set_pixel(0, 0, "ff0000", Some("Bob")).await.unwrap();
        assert!(state.leaderboard().await.iter().any(|e| e.name == "Bob"));

        state.delete_player("Bob").await;
        // Gone from the leaderboard, token revoked, pseudo freed.
        assert!(!state.leaderboard().await.iter().any(|e| e.name == "Bob"));
        assert_eq!(state.player_for_token(&token).await, None);
        assert!(state.register_player("Bob").await.is_ok());
    }

    #[tokio::test]
    async fn blank_maintenance_message_falls_back_to_default() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.maintenance_message = "   ".into();
        s.announcement = "  hello  ".into();
        let applied = state.update_settings(s).await;
        assert_eq!(applied.maintenance_message, default_maintenance_message());
        assert_eq!(applied.announcement.as_str(), "hello"); // trimmed
    }

    #[tokio::test]
    async fn canvas_size_is_clamped() {
        let state = AppState::new(None).await;
        let mut s = state.settings();
        s.width = 4; // below the minimum
        s.height = 9000; // above the maximum
        let applied = state.update_settings(s).await;
        assert_eq!(applied.width, 8);
        assert_eq!(applied.height, 512);
    }

    #[tokio::test]
    async fn admin_is_disabled_without_password() {
        // No ADMIN_PASSWORD in this test's env -> admin features are off.
        let state = AppState::new(None).await;
        assert!(!state.admin_enabled());
        assert!(state.admin_login("anything").await.is_none());
        assert!(!state.admin_check("whatever").await);
    }

    #[tokio::test]
    async fn admin_api_requires_auth() {
        let response = app(AppState::new(None).await)
            .oneshot(
                Request::builder()
                    .uri("/admin/api/state")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn admin_page_is_served() {
        let response = app(AppState::new(None).await)
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn constant_time_eq_matches_only_equal_bytes() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secrev"));
        assert!(!constant_time_eq(b"secret", b"secre"));
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

    #[tokio::test]
    async fn leaderboard_tracks_pixel_counts() {
        let state = AppState::new(None).await;
        assert!(state.set_pixel(0, 0, "ff0000", Some("Alice")).await.is_ok());
        assert!(state.set_pixel(1, 0, "00ff00", Some("Bob")).await.is_ok());
        assert!(state.set_pixel(2, 0, "0000ff", Some("Alice")).await.is_ok());

        let lb = state.leaderboard().await;
        assert_eq!(lb[0].name, "Alice");
        assert_eq!(lb[0].count, 2);
        assert_eq!(lb[1].name, "Bob");
        assert_eq!(lb[1].count, 1);
    }

    #[tokio::test]
    async fn leaderboard_sanitizes_names() {
        let state = AppState::new(None).await;
        assert!(state
            .set_pixel(0, 0, "ff0000", Some("<script>alert(1)</script>"))
            .await
            .is_ok());
        let lb = state.leaderboard().await;
        assert!(!lb[0].name.contains('<'));
        assert!(!lb[0].name.contains('>'));
        assert!(!lb[0].name.contains('/'));
    }

    #[tokio::test]
    async fn leaderboard_endpoint_returns_player() {
        let state = AppState::new(None).await;
        assert!(state
            .set_pixel(0, 0, "ff0000", Some("TestPlayer"))
            .await
            .is_ok());
        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri("/api/leaderboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_string(response).await;
        assert!(body.contains("TestPlayer"), "got: {body}");
    }
}
