# Architecture Decision Records

This directory records the significant architecture decisions made on Pixelflux,
using lightweight [ADRs](https://adr.github.io/). Each file captures the context,
the decision, and its consequences, so newcomers understand *why* things are the
way they are.

Use [`template.md`](template.md) for new records. Number them sequentially and
never rewrite history: to change a decision, add a new ADR that supersedes the
old one.

## Index

| #    | Title                                                       | Status   |
| ---- | ----------------------------------------------------------- | -------- |
| 0001 | [Use Nix for the dev shell and the container build](0001-nix-for-devshell-and-container.md) | Accepted |
| 0002 | [Ship a distroless, non-root, static container](0002-distroless-nonroot-container.md)       | Accepted |
| 0003 | [Back the canvas with Redis and pub/sub](0003-redis-canvas-and-pubsub.md)                   | Accepted |
| 0004 | [Use Server-Sent Events instead of WebSockets](0004-sse-over-websockets.md)                 | Accepted |
| 0005 | [Deploy on Kubernetes behind Traefik](0005-kubernetes-traefik-deployment.md)                | Accepted |
