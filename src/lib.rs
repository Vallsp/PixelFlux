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
//! Live updates are pushed to browsers over Server-Sent Events: every painted
//! pixel is fanned out through an in-process `broadcast` channel to all clients
//! connected to `/api/events`.

use axum::{
    extract::State,
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};

pub const WIDTH: usize = 64;
pub const HEIGHT: usize = 64;
const CANVAS_KEY: &str = "canvas";

/// 16-colour palette (index = hex char stored per pixel).
pub const PALETTE: [&str; 16] = [
    "#ffffff", "#e4e4e4", "#888888", "#222222", "#ffa7d1", "#e50000", "#e59500", "#a06a42",
    "#e5d900", "#94e044", "#02be01", "#00d3dd", "#0083c7", "#0000ea", "#cf6ee4", "#820080",
];

fn blank_canvas() -> Vec<u8> {
    vec![b'0'; WIDTH * HEIGHT]
}

fn hex_char(color: u8) -> Option<u8> {
    std::char::from_digit(color as u32, 16).map(|c| c as u8)
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
    /// (initialised once, atomically, if absent). Any connection error
    /// degrades gracefully to the in-memory canvas.
    pub async fn new(redis_url: Option<String>) -> Self {
        let redis = match redis_url {
            Some(url) => match redis::Client::open(url) {
                Ok(client) => redis::aio::ConnectionManager::new(client).await.ok(),
                Err(_) => None,
            },
            None => None,
        };

        if let Some(conn) = &redis {
            let mut conn = conn.clone();
            let blank = String::from_utf8(blank_canvas()).unwrap();
            let _ = redis::cmd("SETNX")
                .arg(CANVAS_KEY)
                .arg(blank)
                .query_async::<i64>(&mut conn)
                .await;
        }

        let (tx, _rx) = broadcast::channel(1024);

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

    /// Paint a single pixel. Returns `true` if the coordinates and colour were
    /// valid and the change was applied (and broadcast to live clients).
    pub async fn set_pixel(&self, x: usize, y: usize, color: u8) -> bool {
        let ch = match (x < WIDTH, y < HEIGHT, hex_char(color)) {
            (true, true, Some(ch)) => ch,
            _ => return false,
        };
        let offset = y * WIDTH + x;

        let mut applied = false;
        if let Some(conn) = &self.redis {
            let mut conn = conn.clone();
            applied = redis::cmd("SETRANGE")
                .arg(CANVAS_KEY)
                .arg(offset)
                .arg((ch as char).to_string())
                .query_async::<i64>(&mut conn)
                .await
                .is_ok();
        }
        if !applied {
            self.fallback.lock().unwrap()[offset] = ch;
            applied = true;
        }

        // Fan the update out to every connected browser (ignored if none).
        let _ = self
            .tx
            .send(format!(r#"{{"x":{x},"y":{y},"color":{color}}}"#));
        applied
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
) -> (StatusCode, Json<PixelResponse>) {
    let ok = state.set_pixel(req.x, req.y, req.color).await;
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (code, Json(PixelResponse { ok }))
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
        assert!(state.set_pixel(1, 0, 5).await); // offset 1 -> '5'
        assert!(state.set_pixel(0, 1, 10).await); // offset WIDTH -> 'a'

        let canvas = state.canvas().await;
        assert_eq!(canvas.as_bytes()[1], b'5');
        assert_eq!(canvas.as_bytes()[WIDTH], b'a');
    }

    #[tokio::test]
    async fn set_pixel_rejects_invalid_input() {
        let state = AppState::new(None).await;
        assert!(!state.set_pixel(WIDTH, 0, 1).await); // x out of bounds
        assert!(!state.set_pixel(0, HEIGHT, 1).await); // y out of bounds
        assert!(!state.set_pixel(0, 0, 99).await); // colour out of palette
    }

    #[tokio::test]
    async fn painting_emits_a_live_event() {
        let state = AppState::new(None).await;
        let mut rx = state.tx.subscribe();
        assert!(state.set_pixel(3, 4, 7).await);
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg, r#"{"x":3,"y":4,"color":7}"#);
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let response = app(AppState::new(None).await)
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
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
