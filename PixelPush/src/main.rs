//! pixelpush — paint an image onto a PixelFlux canvas, as fast as a per-pixel
//! HTTP API allows.
//!
//! Reads the canvas dimensions and palette from the server at runtime (so it
//! auto-adapts to any size / palette), quantises an image to that palette in
//! CIE Lab, then fires the POSTs with **async I/O**: a single pooled
//! `reqwest::Client` (keep-alive + HTTP/2 multiplexing on TLS) with hundreds of
//! requests in flight, bounded by a semaphore. Each pass only paints the pixels
//! that differ from the live canvas; `--bruteforce` repeats to hold the image.
//!
//! The paint endpoint is token-gated (`x-token` header, rate-limited per token),
//! so pixelpush registers a pool of tokens up front and a client-side rate gate
//! spreads paints across them, staying under the server's per-token budget.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use image::{imageops::FilterType, Rgba, RgbaImage};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;

/// Upper bound on the auto-sized token pool, so we never register an absurd
/// number of tokens for a huge image. Override with an explicit `--tokens`.
const MAX_AUTO_TOKENS: usize = 16;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

struct Args {
    image: String,
    host: String,
    contain: bool,
    nearest: bool,
    dither: bool,
    alpha_skip: bool,
    alpha_threshold: u8,
    background: [u8; 3],
    concurrency: usize,
    connections: usize,
    delay_ms: u64,
    bruteforce: bool,
    interval_ms: u64,
    tokens: Option<usize>,
    supplied_tokens: Vec<String>,
    rate: u64,
    rate_window: u64,
}

struct TokenPool {
    entries: Vec<TokenEntry>,
    next: AtomicUsize,
}

struct TokenEntry {
    token: String,
    gate: Mutex<TokenGate>,
}

struct TokenGate {
    next_at: Instant,
    spacing: Duration,
}

impl TokenPool {
    fn new(tokens: Vec<String>, rate: u64, rate_window: Duration) -> Result<Self, String> {
        if tokens.is_empty() {
            return Err("no paint tokens available".into());
        }
        if rate == 0 || rate_window.is_zero() {
            return Err("--rate and --rate-window must be >= 1".into());
        }

        // Keep one slot of margin so boundary timing does not trip the server's
        // per-token rolling window.
        let intervals = rate.saturating_sub(1).max(1);
        let spacing = Duration::from_secs_f64(rate_window.as_secs_f64() / intervals as f64);
        let now = Instant::now();
        let entries = tokens
            .into_iter()
            .map(|token| TokenEntry {
                token,
                gate: Mutex::new(TokenGate {
                    next_at: now,
                    spacing,
                }),
            })
            .collect();

        Ok(Self {
            entries,
            next: AtomicUsize::new(0),
        })
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    async fn claim(&self) -> String {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.entries.len();
        let entry = &self.entries[index];
        let mut gate = entry.gate.lock().await;
        let now = Instant::now();
        if gate.next_at > now {
            tokio::time::sleep(gate.next_at.duration_since(now)).await;
        }
        gate.next_at = Instant::now() + gate.spacing;
        entry.token.clone()
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let host = args.host.trim_end_matches('/').to_string();

    // One pooled client per connection. reqwest keeps a single HTTP/2 connection
    // per host, capped by the server's ~100–128 max-concurrent-streams; sharding
    // across N clients = N connections, which exceeds that cap and avoids TCP
    // head-of-line blocking. Each client keep-alives its connection(s).
    let connections = args.connections.max(1);
    let clients: Vec<reqwest::Client> = (0..connections)
        .map(|_| {
            reqwest::Client::builder()
                .pool_max_idle_per_host(args.concurrency)
                .timeout(Duration::from_secs(20))
                .build()
        })
        .collect::<Result<_, _>>()
        .map_err(|e| format!("http client: {e}"))?;

    // 1. Canvas metadata — dimensions + palette, straight from the server.
    let canvas_url = format!("{host}/api/canvas");
    let pixel_url: Arc<str> = Arc::from(format!("{host}/api/pixel"));
    let meta = fetch_json(&clients[0], &canvas_url).await?;

    let width = meta["width"].as_u64().ok_or("canvas: missing 'width'")? as u32;
    let height = meta["height"].as_u64().ok_or("canvas: missing 'height'")? as u32;
    let palette: Vec<[u8; 3]> = meta["palette"]
        .as_array()
        .ok_or("canvas: missing 'palette'")?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| "palette entry is not a string".to_string())
                .and_then(parse_hex)
        })
        .collect::<Result<_, _>>()?;
    if width == 0 || height == 0 || palette.is_empty() {
        return Err("canvas reported an empty grid or palette".into());
    }
    eprintln!(
        "canvas {width}×{height} · {} colours · {host} · {connections} conn × {} in-flight",
        palette.len(),
        args.concurrency
    );

    // Pre-convert the palette to CIE Lab for perceptual colour matching.
    let pal_lab: Vec<[f32; 3]> = palette.iter().map(|&c| srgb_to_lab(c)).collect();

    // 2. Load + resize the image, then 3. quantise to palette indices.
    let src = image::open(&args.image)
        .map_err(|e| format!("open {}: {e}", args.image))?
        .to_rgba8();
    let resized = resize_to(&src, width, height, args.contain, args.nearest);
    let grid = quantise(
        &resized,
        &pal_lab,
        args.dither,
        args.alpha_skip,
        args.alpha_threshold,
        args.background,
    );

    let target = grid.iter().filter(|c| c.is_some()).count();
    if target == 0 {
        eprintln!("nothing to paint (every pixel skipped).");
        return Ok(());
    }
    if args.bruteforce {
        eprintln!(
            "bruteforce — holding {target} pixels, re-checking every {} ms (Ctrl-C to stop)",
            args.interval_ms
        );
    }

    // Decide how many tokens to register. An explicit --tokens wins; otherwise
    // auto-size to repaint the whole image within one rate window (capped), so
    // the default stays fast under the per-token limit instead of pacing a
    // single token for minutes.
    let auto_count = match args.tokens {
        Some(n) => n,
        None if !args.supplied_tokens.is_empty() => 0,
        None => ((target as u64).div_ceil(args.rate.max(1)) as usize).clamp(1, MAX_AUTO_TOKENS),
    };
    let tokens = collect_tokens(&clients[0], &host, auto_count, &args.supplied_tokens).await?;
    let token_pool = Arc::new(TokenPool::new(
        tokens,
        args.rate,
        Duration::from_secs(args.rate_window),
    )?);
    eprintln!(
        "using {} paint token(s) · {} paint(s)/{}s each → ~{:.0} px/s sustained",
        token_pool.len(),
        args.rate,
        args.rate_window,
        token_pool.len() as f64 * args.rate as f64 / args.rate_window.max(1) as f64,
    );

    // 4. Each pass: diff the live canvas against the target, repaint the misses.
    let mut pass = 0u64;
    loop {
        pass += 1;
        let pixels = match fetch_json(&clients[0], &canvas_url).await {
            Ok(v) => v["pixels"].as_str().unwrap_or("").to_string(),
            Err(e) => {
                eprintln!("pass {pass}: {e}");
                if args.bruteforce {
                    tokio::time::sleep(Duration::from_millis(args.interval_ms.max(250))).await;
                    continue;
                }
                return Err(e);
            }
        };
        let bytes = pixels.as_bytes();

        let mut todo: Vec<(u32, u32, u8)> = Vec::new();
        for (i, cell) in grid.iter().enumerate() {
            if let Some(c) = *cell {
                let cur = bytes
                    .get(i)
                    .and_then(|b| (*b as char).to_digit(16))
                    .map(|d| d as u8);
                if cur != Some(c) {
                    todo.push(((i as u32) % width, (i as u32) / width, c));
                }
            }
        }

        let start = Instant::now();
        let errs = paint(
            &clients,
            &pixel_url,
            token_pool.clone(),
            todo.clone(),
            args.concurrency,
            args.delay_ms,
        )
        .await;
        let fixed = todo.len().saturating_sub(errs);
        let secs = start.elapsed().as_secs_f64();

        if !args.bruteforce {
            let rate = if secs > 0.0 { fixed as f64 / secs } else { 0.0 };
            eprintln!(
                "painted {fixed} changed pixel(s) ({errs} err) in {secs:.2}s ({rate:.0}/s); {} already correct",
                target - todo.len()
            );
            return Ok(());
        }

        eprintln!(
            "pass {pass}: {} off → painted {fixed} ({errs} err) in {secs:.2}s",
            todo.len()
        );
        tokio::time::sleep(Duration::from_millis(args.interval_ms)).await;
    }
}

async fn collect_tokens(
    client: &reqwest::Client,
    host: &str,
    auto_count: usize,
    supplied: &[String],
) -> Result<Vec<String>, String> {
    let mut tokens = supplied.to_vec();
    if auto_count > 0 {
        // Registration is deliberately slow (~5s server-side) to make minting
        // tokens en masse expensive — but the delay is just a sleep, so firing
        // all registrations at once collapses N×5s into ~5s total.
        eprintln!(
            "registering {auto_count} paint token(s) — the server adds a ~5s anti-abuse delay each (done in parallel)…"
        );
        let register_url = format!("{host}/register");
        let mut set: JoinSet<Result<String, String>> = JoinSet::new();
        for _ in 0..auto_count {
            let client = client.clone();
            let url = register_url.clone();
            set.spawn(async move { register_token(&client, &url).await });
        }
        while let Some(joined) = set.join_next().await {
            tokens.push(joined.map_err(|e| format!("register task failed: {e}"))??);
        }
    }
    if tokens.is_empty() {
        return Err("no paint tokens available; pass --token or allow --tokens 1".into());
    }
    Ok(tokens)
}

async fn register_token(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let value = client
        .post(url)
        .send()
        .await
        .map_err(|e| format!("POST {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("POST {url}: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse {url}: {e}"))?;

    value["token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "register: response missing 'token'".to_string())
}

/// GET a URL and parse it as JSON.
async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value, String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GET {url}: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse {url}: {e}"))
}

/// Shard the work across `clients` (one connection each) and paint all shards
/// in parallel; `per_conn` bounds in-flight requests per connection. Returns the
/// number of failed pixels.
async fn paint(
    clients: &[reqwest::Client],
    url: &Arc<str>,
    token_pool: Arc<TokenPool>,
    list: Vec<(u32, u32, u8)>,
    per_conn: usize,
    delay: u64,
) -> usize {
    if list.is_empty() {
        return 0;
    }
    let n = clients.len().max(1);
    let chunk = list.len().div_ceil(n);
    let mut shards: JoinSet<usize> = JoinSet::new();
    for (i, client) in clients.iter().enumerate() {
        let lo = i * chunk;
        if lo >= list.len() {
            break;
        }
        let hi = ((i + 1) * chunk).min(list.len());
        let shard = list[lo..hi].to_vec();
        let client = client.clone();
        let url = url.clone();
        let token_pool = token_pool.clone();
        shards.spawn(async move {
            paint_shard(&client, &url, token_pool, shard, per_conn, delay).await
        });
    }
    let mut errors = 0usize;
    while let Some(joined) = shards.join_next().await {
        errors += joined.unwrap_or(0);
    }
    errors
}

/// Paint one shard over a single connection, up to `conc` requests in flight.
async fn paint_shard(
    client: &reqwest::Client,
    url: &Arc<str>,
    token_pool: Arc<TokenPool>,
    list: Vec<(u32, u32, u8)>,
    conc: usize,
    delay: u64,
) -> usize {
    let sem = Arc::new(Semaphore::new(conc.max(1)));
    let mut set: JoinSet<bool> = JoinSet::new();
    for (x, y, c) in list {
        let permit = sem.clone().acquire_owned().await.expect("semaphore closed");
        let client = client.clone();
        let url = url.clone();
        let token_pool = token_pool.clone();
        set.spawn(async move {
            let _permit = permit; // released when this request finishes
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let token = token_pool.claim().await;
            let body = serde_json::json!({ "x": x, "y": y, "color": c });
            matches!(
                client
                    .post(url.as_ref())
                    .header("X-Token", token)
                    .json(&body)
                    .send()
                    .await,
                Ok(r) if r.status().is_success()
            )
        });
    }
    let mut errors = 0usize;
    while let Some(joined) = set.join_next().await {
        if !joined.unwrap_or(false) {
            errors += 1;
        }
    }
    errors
}

fn resize_to(img: &RgbaImage, w: u32, h: u32, contain: bool, nearest: bool) -> RgbaImage {
    let filter = if nearest {
        FilterType::Nearest
    } else {
        FilterType::Lanczos3
    };
    let (tw, th) = if contain {
        // Preserve aspect ratio: fit inside w×h.
        let (iw, ih) = img.dimensions();
        let scale = (w as f32 / iw as f32).min(h as f32 / ih as f32);
        (
            ((iw as f32 * scale).round() as u32).clamp(1, w),
            ((ih as f32 * scale).round() as u32).clamp(1, h),
        )
    } else {
        (w, h)
    };

    // Premultiply alpha before resizing so transparent (often black) pixels
    // don't bleed colour into the edges; un-premultiply afterwards.
    let mut pm = img.clone();
    premultiply(&mut pm);
    let mut small = image::imageops::resize(&pm, tw, th, filter);
    unpremultiply(&mut small);

    if !contain {
        return small;
    }
    // Place (don't alpha-blend) onto a transparent canvas, centred.
    let mut canvas = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
    image::imageops::replace(
        &mut canvas,
        &small,
        ((w - tw) / 2) as i64,
        ((h - th) / 2) as i64,
    );
    canvas
}

fn premultiply(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p.0[3] as u16;
        p.0[0] = (p.0[0] as u16 * a / 255) as u8;
        p.0[1] = (p.0[1] as u16 * a / 255) as u8;
        p.0[2] = (p.0[2] as u16 * a / 255) as u8;
    }
}

fn unpremultiply(img: &mut RgbaImage) {
    for p in img.pixels_mut() {
        let a = p.0[3] as u16;
        if let (Some(r), Some(g), Some(b)) = (
            unpremultiply_channel(p.0[0], a),
            unpremultiply_channel(p.0[1], a),
            unpremultiply_channel(p.0[2], a),
        ) {
            p.0[0] = r;
            p.0[1] = g;
            p.0[2] = b;
        }
    }
}

fn unpremultiply_channel(value: u8, alpha: u16) -> Option<u8> {
    (value as u16 * 255)
        .checked_div(alpha)
        .map(|value| value.min(255) as u8)
}

/// Map every cell to a palette index, in CIE Lab so matches are perceptual.
/// Semi-transparent pixels are composited over `background` (in linear light)
/// unless `alpha_skip` leaves them untouched (`None`).
fn quantise(
    img: &RgbaImage,
    pal_lab: &[[f32; 3]],
    dither: bool,
    alpha_skip: bool,
    threshold: u8,
    background: [u8; 3],
) -> Vec<Option<u8>> {
    let (w, h) = img.dimensions();
    let bg_lin = [
        srgb_to_linear(background[0]),
        srgb_to_linear(background[1]),
        srgb_to_linear(background[2]),
    ];

    let mut buf: Vec<[f32; 3]> = img.pixels().map(|p| pixel_to_lab(p.0, bg_lin)).collect();
    let alpha: Vec<u8> = img.pixels().map(|p| p.0[3]).collect();
    let mut out: Vec<Option<u8>> = vec![None; (w * h) as usize];

    if !dither {
        for i in 0..buf.len() {
            if alpha_skip && alpha[i] < threshold {
                continue;
            }
            out[i] = Some(nearest_lab(buf[i], pal_lab));
        }
        return out;
    }

    let (wi, hi) = (w as i64, h as i64);
    let diffuse = |buf: &mut [[f32; 3]], xx: i64, yy: i64, f: f32, err: [f32; 3]| {
        if xx >= 0 && xx < wi && yy >= 0 && yy < hi {
            let j = (yy as usize) * (w as usize) + (xx as usize);
            buf[j][0] += err[0] * f;
            buf[j][1] += err[1] * f;
            buf[j][2] += err[2] * f;
        }
    };
    for y in 0..hi {
        for x in 0..wi {
            let i = (y as usize) * (w as usize) + (x as usize);
            if alpha_skip && alpha[i] < threshold {
                continue;
            }
            let old = buf[i];
            let ci = nearest_lab(old, pal_lab);
            out[i] = Some(ci);
            let np = pal_lab[ci as usize];
            let err = [old[0] - np[0], old[1] - np[1], old[2] - np[2]];
            diffuse(&mut buf, x + 1, y, 7.0 / 16.0, err);
            diffuse(&mut buf, x - 1, y + 1, 3.0 / 16.0, err);
            diffuse(&mut buf, x, y + 1, 5.0 / 16.0, err);
            diffuse(&mut buf, x + 1, y + 1, 1.0 / 16.0, err);
        }
    }
    out
}

/// Nearest palette entry by Euclidean distance in Lab (ΔE76).
fn nearest_lab(lab: [f32; 3], pal_lab: &[[f32; 3]]) -> u8 {
    let mut best = 0usize;
    let mut best_d = f32::MAX;
    for (i, p) in pal_lab.iter().enumerate() {
        let dl = lab[0] - p[0];
        let da = lab[1] - p[1];
        let db = lab[2] - p[2];
        let d = dl * dl + da * da + db * db;
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best as u8
}

// --- Colour conversion: sRGB → linear → CIE Lab (D65) ----------------------

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_lab(r: f32, g: f32, b: f32) -> [f32; 3] {
    let x = r * 0.4124564 + g * 0.3575761 + b * 0.1804375;
    let y = r * 0.2126729 + g * 0.7151522 + b * 0.0721750;
    let z = r * 0.0193339 + g * 0.119_192 + b * 0.9503041;
    let f = |t: f32| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / 0.95047), f(y), f(z / 1.08883));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn srgb_to_lab(rgb: [u8; 3]) -> [f32; 3] {
    linear_to_lab(
        srgb_to_linear(rgb[0]),
        srgb_to_linear(rgb[1]),
        srgb_to_linear(rgb[2]),
    )
}

/// Composite an RGBA pixel over `bg_lin` (linear light), then convert to Lab.
fn pixel_to_lab(p: [u8; 4], bg_lin: [f32; 3]) -> [f32; 3] {
    let a = p[3] as f32 / 255.0;
    let r = srgb_to_linear(p[0]) * a + bg_lin[0] * (1.0 - a);
    let g = srgb_to_linear(p[1]) * a + bg_lin[1] * (1.0 - a);
    let b = srgb_to_linear(p[2]) * a + bg_lin[2] * (1.0 - a);
    linear_to_lab(r, g, b)
}

fn parse_hex(s: &str) -> Result<[u8; 3], String> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return Err(format!("bad hex colour: '{s}'"));
    }
    let byte = |r: std::ops::Range<usize>| {
        u8::from_str_radix(&s[r], 16).map_err(|_| format!("bad hex colour: '{s}'"))
    };
    Ok([byte(0..2)?, byte(2..4)?, byte(4..6)?])
}

fn parse_args() -> Result<Args, String> {
    let mut image: Option<String> = None;
    let mut host = "http://localhost:3000".to_string();
    let mut contain = false;
    let mut nearest = false;
    let mut dither = false;
    let mut alpha_skip = true; // respect transparency by default
    let mut alpha_threshold = 128u8;
    let mut background = [255u8, 255, 255]; // composite colour for kept partial-alpha pixels
    let mut concurrency = 128usize;
    let mut connections = 1usize;
    let mut delay_ms = 0u64;
    let mut bruteforce = false;
    let mut interval_ms = 1000u64;
    let mut auto_tokens: Option<usize> = None;
    let mut supplied_tokens = Vec::new();
    let mut rate = 4096u64;
    let mut rate_window = 30u64;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let need = |it: &mut std::iter::Skip<std::env::Args>, flag: &str| {
            it.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        match a.as_str() {
            "--host" => host = need(&mut it, "--host")?,
            "--contain" => contain = true,
            "--nearest" => nearest = true,
            "--dither" => dither = true,
            "--alpha-skip" => alpha_skip = true, // back-compat: now the default
            "--flatten" => alpha_skip = false,
            "--alpha-threshold" => {
                alpha_threshold = need(&mut it, "--alpha-threshold")?
                    .parse()
                    .map_err(|_| "--alpha-threshold must be 0–255")?
            }
            "--background" => background = parse_hex(&need(&mut it, "--background")?)?,
            "--token" => {
                let token = need(&mut it, "--token")?.trim().to_string();
                if token.is_empty() {
                    return Err("--token must not be empty".into());
                }
                supplied_tokens.push(token);
            }
            "--tokens" => {
                auto_tokens = Some(
                    need(&mut it, "--tokens")?
                        .parse()
                        .map_err(|_| "--tokens must be a number")?,
                );
            }
            "--rate" => {
                rate = need(&mut it, "--rate")?
                    .parse()
                    .map_err(|_| "--rate must be a number")?
            }
            "--rate-window" => {
                rate_window = need(&mut it, "--rate-window")?
                    .parse()
                    .map_err(|_| "--rate-window must be a number")?
            }
            "--bruteforce" | "--loop" => bruteforce = true,
            "--interval-ms" => {
                interval_ms = need(&mut it, "--interval-ms")?
                    .parse()
                    .map_err(|_| "--interval-ms must be a number")?
            }
            "--concurrency" => {
                concurrency = need(&mut it, "--concurrency")?
                    .parse()
                    .map_err(|_| "--concurrency must be a number")?
            }
            "--connections" => {
                connections = need(&mut it, "--connections")?
                    .parse()
                    .map_err(|_| "--connections must be a number")?
            }
            "--delay-ms" => {
                delay_ms = need(&mut it, "--delay-ms")?
                    .parse()
                    .map_err(|_| "--delay-ms must be a number")?
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            s if s.starts_with('-') => return Err(format!("unknown flag: {s}")),
            s => {
                if image.is_some() {
                    return Err("more than one image path given".into());
                }
                image = Some(s.to_string());
            }
        }
    }

    if concurrency == 0 || connections == 0 {
        return Err("--concurrency and --connections must be >= 1".into());
    }
    if rate == 0 || rate_window == 0 {
        return Err("--rate and --rate-window must be >= 1".into());
    }
    if auto_tokens == Some(0) && supplied_tokens.is_empty() {
        return Err("no paint tokens requested; pass --token or use --tokens 1".into());
    }
    let image = image.ok_or("missing <image> argument (try --help)")?;
    Ok(Args {
        image,
        host,
        contain,
        nearest,
        dither,
        alpha_skip,
        alpha_threshold,
        background,
        concurrency,
        connections,
        delay_ms,
        bruteforce,
        interval_ms,
        tokens: auto_tokens,
        supplied_tokens,
        rate,
        rate_window,
    })
}

fn print_help() {
    eprintln!(
        "pixelpush — paint an image onto a PixelFlux canvas

USAGE:
    pixelpush <image> [options]

OPTIONS:
    --host <url>       server base URL (default http://localhost:3000)
    --contain          preserve aspect ratio (letterbox); default stretches to fill
    --nearest          nearest-neighbour resize (good for pixel art); default Lanczos3
    --dither           Floyd–Steinberg dithering (better for photos)
    --flatten          paint transparent pixels too, filling with --background;
                       by default transparent pixels are left untouched
    --alpha-threshold <n>  alpha 0–255 below which a pixel is treated as
                       transparent and skipped (default 128)
    --background <hex> composite colour for kept partial-alpha pixels, and the
                       fill colour under --flatten (default ffffff)
    --token <value>    reuse an existing paint token; repeat for a token pool
                       (skips registration; combine with --tokens 0)
    --tokens <n>       register n paint tokens up front, in parallel. Default:
                       auto — enough to repaint the image within one rate window
                       (capped at 16); use 0 to register none
    --rate <n>         max paints per token per rate window (default 4096)
    --rate-window <s>  rate window in seconds (default 30)
    --concurrency <n>  max requests in flight per connection (default 128)
    --connections <n>  parallel connections to shard across (default 1); raise
                       on the TLS board to beat the ~128 streams/connection cap
    --delay-ms <n>     delay before each request (default 0)
    --bruteforce       keep the image up: loop forever, re-painting any pixel
                       others change (alias: --loop; Ctrl-C to stop)
    --interval-ms <n>  pause between bruteforce passes (default 1000)
    -h, --help         show this help

POSTs run async (keep-alive + HTTP/2 on TLS), sharded across --connections
clients with --concurrency requests in flight each (≈ conn × concurrency total).
Each run diffs the live canvas and only paints the pixels that differ. The
canvas size and palette are read from the server at runtime, so the same binary
works for any grid dimensions or palette."
    );
}
