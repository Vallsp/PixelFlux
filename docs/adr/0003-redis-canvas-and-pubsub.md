# 0003. Back the canvas with Redis and pub/sub

- **Status:** Accepted
- **Date:** 2026-06-19

## Context

The canvas is shared: every visitor draws on the same grid, and the app runs as
several replicas behind a load balancer. State therefore cannot live in a single
process's memory, and a pixel painted on one replica must reach users connected
to any other replica in real time.

## Decision

Store the canvas in **Redis** as a single `width*height` string, mutated with
`SETRANGE` and read with `GET`. Propagate live updates with **Redis pub/sub**:
each painted pixel is `PUBLISH`ed, and every replica subscribes and fans the
event out to its own connected browsers. If no `REDIS_URL` is configured, the app
falls back to an in-memory canvas and an in-process broadcast (single instance).

## Consequences

### Positive

- Shared, consistent state across all replicas; horizontal scaling just works.
- Real-time updates reach users on any replica, no sticky sessions needed.
- Graceful local development with zero dependencies (in-memory fallback).

### Trade-offs / negative

- Redis is an additional component to run and operate.
- The current Redis deployment is ephemeral (no persistence); a restart clears
  the canvas. Acceptable for now; a `StatefulSet` + PVC would fix it.

### Alternatives considered

- **In-memory only** — simplest, but breaks as soon as there is more than one
  replica.
- **A SQL database** — overkill for a single mutable blob and a fan-out channel.
