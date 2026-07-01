# API

Pixelflux exposes a small HTTP API. The full machine-readable spec is in
[`openapi.yaml`](openapi.yaml); [`contract.hurl`](contract.hurl) is the
executable contract test suite.

## Endpoints

| Method | Route              | Description                                                                                                                                                       |
| ------ | ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GET    | `/`                | Web UI (embedded single page)                                                                                                                                     |
| GET    | `/health`          | Liveness probe → `{"status":"ok"}`                                                                                                                                |
| GET    | `/info`            | `{"name", "version", "instance", "online"}` — `instance` is the serving pod/host; `online` is the live viewer count (open SSE streams)                            |
| GET    | `/api/canvas`      | Whole canvas → `{"width", "height", "palette", "pixels"}` (`pixels` is a `width*height*6` hex string — an `rrggbb` colour per cell)                               |
| POST   | `/register`        | Register a **unique pseudo** → `{"token", "name"}`. The pseudo is bound to the token server-side. Slow (~5s) anti-abuse (400 invalid · 409 taken · 503 closed)    |
| POST   | `/api/pixel`       | Paint one pixel → header `X-Token` + body `{"x", "y", "color"}` (`color` = `rrggbb` hex) → `{"ok": true}` (400 invalid · 401 no/unknown token · 429 rate limited) |
| GET    | `/api/events`      | Live pixel stream (SSE); coalesced **batches** `[{"x","y","color"}, …]` + a named `leaderboard` event (top-10)                                                    |
| GET    | `/api/leaderboard` | Top-10 players by **pixels painted** (cumulative) → `[{"name","count"}, …]`                                                                                       |
| GET    | `/api/ownership`   | **Territory** — pixels each player currently owns → `{"total", "entries":[{"name","count","percent"}]}` (transfers when a pixel is overwritten)                   |
| GET    | `/admin`           | Admin dashboard (enabled only when `ADMIN_PASSWORD` is set); tune limits, maintenance mode, reset canvas, live stats                                              |

The canvas is **200×200** in **full RGB** — each pixel is any `rrggbb` colour
(16M colours); the `palette` field just gives default preset swatches for the UI.
Painting requires a token from `/register` (sent as `X-Token`), and is rate
limited (default **4096 pixels per token per 30 s**). The rate limit, window and
other tunables are editable at runtime from the admin page; when maintenance mode
is on, painting returns **503**.

Each player registers a **unique pseudo**, which is bound to their token
server-side. The leaderboard credit is derived from the token — not from the
paint request body — so a client can't paint under someone else's name.

## Try it

```bash
curl localhost:3000/health
curl localhost:3000/api/canvas
curl -X POST localhost:3000/api/pixel \
  -H 'Content-Type: application/json' \
  -d '{"x":1,"y":2,"color":5}'
curl -N localhost:3000/api/events     # streams pixel events as they happen
```

## Contract tests

`contract.hurl` validates the live server against the spec (status codes,
content types, JSON assertions, including the 400 path for an invalid pixel).
Run it with:

```bash
task test:api
```

The task builds and boots the server, runs Hurl against it, and shuts it down.
To run it against an already-running server:

```bash
hurl --test --variable host=http://localhost:3000 api/contract.hurl
```

## Editing the API

When you add or change an endpoint, update **both** `openapi.yaml` (the spec)
and `contract.hurl` (the test), then run `task test:api` to keep them in sync.
