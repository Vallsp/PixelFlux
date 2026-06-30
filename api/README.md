# API

Pixelflux exposes a small HTTP API. The full machine-readable spec is in
[`openapi.yaml`](openapi.yaml); [`contract.hurl`](contract.hurl) is the
executable contract test suite.

## Endpoints

| Method | Route         | Description                                                                                                                                                       |
| ------ | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GET    | `/`           | Web UI (embedded single page)                                                                                                                                     |
| GET    | `/health`     | Liveness probe → `{"status":"ok"}`                                                                                                                                |
| GET    | `/info`       | `{"name", "version", "instance", "online"}` — `instance` is the serving pod/host; `online` is the live viewer count (open SSE streams)                            |
| GET    | `/api/canvas` | Whole canvas → `{"width", "height", "palette", "pixels"}` (`pixels` is a `width*height*6` hex string — an `rrggbb` colour per cell)                               |
| POST   | `/register`   | Issue a paint token → `{"token"}`. Deliberately slow (~5s) to make mass token creation expensive                                                                  |
| POST   | `/api/pixel`  | Paint one pixel → header `X-Token` + body `{"x", "y", "color"}` (`color` = `rrggbb` hex) → `{"ok": true}` (400 invalid · 401 no/unknown token · 429 rate limited) |
| GET    | `/api/events` | Live pixel stream (SSE); each event is a coalesced **batch** — a JSON array `[{"x","y","color"}, …]` flushed on a tick (default 16 ms, `SSE_COALESCE_MS`)         |

The canvas is **200×200** in **full RGB** — each pixel is any `rrggbb` colour
(16M colours); the `palette` field just gives default preset swatches for the UI.
Painting requires a token from `/register` (sent as `X-Token`), and is limited to
**4096 pixels per token per 30 s**.

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
