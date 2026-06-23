# API

Pixelflux exposes a small HTTP API. The full machine-readable spec is in
[`openapi.yaml`](openapi.yaml); [`contract.hurl`](contract.hurl) is the
executable contract test suite.

## Endpoints

| Method | Route | Description |
| --- | --- | --- |
| GET | `/` | Web UI (embedded single page) |
| GET | `/health` | Liveness probe → `{"status":"ok"}` |
| GET | `/info` | `{"name", "version", "instance"}` — `instance` is the pod/host serving the request (makes load balancing visible) |
| GET | `/api/canvas` | Whole canvas → `{"width", "height", "palette", "pixels"}` (`pixels` is a `width*height` hex string, one palette index per cell) |
| POST | `/api/pixel` | Paint one pixel → body `{"x", "y", "color"}` → `{"ok": true}` (400 if out of bounds or invalid colour) |
| GET | `/api/events` | Live pixel stream (Server-Sent Events); each event's data is `{"x","y","color"}` |

The canvas is **64×64** with a **16-colour palette** (`color` is `0..15`).

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
