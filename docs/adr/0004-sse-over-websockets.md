# 0004. Use Server-Sent Events instead of WebSockets

- **Status:** Accepted
- **Date:** 2026-06-19

## Context

Browsers need to receive live pixel updates. The traffic is almost entirely
**one-way** (server → browser): the only client→server action is painting a
pixel, which is a plain `POST`. We want the lightest mechanism that works well
through a reverse proxy / load balancer.

## Decision

Use **Server-Sent Events** (`GET /api/events`) for the live stream, and a normal
`POST /api/pixel` for writes. SSE is built into axum, needs no extra dependency,
auto-reconnects in the browser (`EventSource`), and streams cleanly through
Traefik. The browser also does a periodic full resync as a safety net.

## Consequences

### Positive

- No heavy WebSocket dependency; smaller binary and simpler code.
- Native browser reconnection; plays well with HTTP proxies and load balancers.
- A good fit for the one-way, broadcast-style update pattern.

### Trade-offs / negative

- SSE is one-directional; a future need for low-latency client→server streaming
  would require revisiting this (e.g. WebSockets).
- Long-lived connections require the proxy to allow streaming (Traefik does by
  default).

### Alternatives considered

- **WebSockets** — bidirectional and powerful, but unnecessary here and adds a
  dependency and more moving parts.
- **Polling** — simplest, but higher latency and wasteful; kept only as the
  resync safety net.
