---
marp: true
title: "PixelFlux — architecture & SDLC"
theme: pixelflux
paginate: true
---

<!-- _class: lead -->
<!-- _paginate: false -->

# PixelFlux

**A real-time collaborative pixel canvas — wrapped in a full production SDLC.**

Rust · axum · Redis · SSE · Nix · Kubernetes · GitOps

<!-- Note: One-line pitch — PixelFlux is a small multiplayer pixel canvas, but the real work is the production pipeline around it: reproducible builds, layered tests, supply-chain scanning, GitOps delivery, and living docs. Plan for the talk: show the app working, then walk through how it is built, tested, secured, shipped, and documented. -->

---

# What it is

- A multiplayer **64×64 pixel canvas**: pick a colour, paint a cell, everyone sees it instantly.
- **Rust / axum** backend; the canvas lives in **Redis**; live updates over **Server-Sent Events**.
- Deliberately small — the app is the vehicle, the **engineering pipeline is the point**.
- One repository holds it all: code, infrastructure, documentation, and these slides.

<!-- Note: The feature set is intentionally tiny so nothing distracts from the engineering. A 64×64 grid, sixteen colours, real-time sync. Keeping the domain trivial means the build system, the tests, the security scans, and the delivery pipeline are what actually get judged. -->

---

# Live demo

- Deployed on **k3s behind Traefik**, served over **HTTPS** (Let's Encrypt).
- Paint in one browser tab → it appears in another **instantly**.
- The footer shows the **app version** and **which pod** answered — refresh and it changes.
- That cross-tab, cross-pod sync is the whole technical story in one gesture.

<!-- Note: Run this live. Open two tabs side by side, paint in one, and watch the pixel land in the other. Then point at the footer: the version comes straight from the build, and the "served by" pod id changes between refreshes because Traefik load-balances across three replicas. If the network misbehaves, fall back to a screenshot. -->

---

# Architecture

```text
Browser ──HTTP──▶  axum  ──┐
   ▲                       ├──▶  Redis   (canvas state + pub/sub)
   └────SSE────── axum ◀───┘
        every replica subscribes; edits fan out to all clients
```

- **Stateless fronts** — any replica serves any request; scale out freely.
- **Redis** is the single source of truth _and_ the event bus.

<!-- Note: The axum servers hold no state, so they scale horizontally under an autoscaler. All canvas state and all events flow through Redis: a paint request writes the pixel and publishes an event, every replica is subscribed, and each pushes that event to its own connected browsers. That is how a paint on pod A reaches a viewer on pod B. -->

---

# Data model & API

- Canvas = **4096 cells**, each a 4-bit index into a **16-colour palette**, held in Redis.
- A small, explicit HTTP surface:
  - `GET /api/canvas` — full snapshot · `POST /api/pixel` — paint one cell
  - `GET /api/events` — **SSE** stream of live edits
  - `GET /health` · `GET /info` — name, version, instance
- The UI is a single embedded `index.html` — no bundler, no framework.

<!-- Note: The whole canvas is tiny, so a snapshot is cheap: a new client fetches /api/canvas once, then follows /api/events for deltas. /info exposes the version and pod id shown in the footer. The frontend is one embedded HTML file on purpose — it keeps the container small and the app easy to audit. -->

---

# Real-time fan-out

- **SSE, not WebSockets** (ADR 0004): one-way server→client is all we need.
- It rides plain HTTP, reconnects on its own, and passes cleanly through proxies.
- A paint: `POST /api/pixel` → write Redis → **publish** → every replica → SSE to its clients.
- New or lagging clients **resync** with a full `GET /api/canvas`.

<!-- Note: The app only ever pushes from server to client, so SSE fits better than WebSockets — simpler, proxy-friendly, and self-healing. Redis pub/sub is what makes multi-instance correct: without it, a paint would only reach clients on the same pod. The reasoning is recorded as ADR 0004 in the repo. -->

---

# Reproducible builds with Nix

- `nix develop` → the **entire toolbox**, pinned and identical for every machine and for CI.
- No "works on my machine": compiler, linters, scanners, k6, mdBook — all from the flake.
- Locked end to end: **`flake.lock`** (tools) plus **`Cargo.lock`** (crates).
- One entrypoint for actions: **`task`** (go-task) — `build`, `run`, `test`, `deploy`, …

<!-- Note: Reproducibility is the foundation everything else stands on. The Nix flake pins every tool to an exact version, so a laptop and the CI runner build the same way. `task` sits on top as a friendly, discoverable command list. Nothing in the project depends on a globally installed tool. -->

---

# Distroless container

- Image **built by Nix**, not a Dockerfile — deterministic layers, no drift.
- **Distroless and non-root** (ADR 0002): no shell, no package manager, tiny attack surface.
- **Under 20 MB**, enforced by a CI size gate that fails the build if it grows.

<!-- Note: The container comes out of the same flake, so there is no separate Dockerfile to rot. Distroless means there is literally no shell or package manager inside, which removes most of what an attacker would reach for, and it runs as a non-root user. A CI check fails the moment the image crosses 20 MB, which keeps us honest about what ships. -->

---

# Testing — four levels

- **Unit** — pure canvas logic (`cargo test --lib`).
- **Integration** — against a **real Redis** via Testcontainers (ADR 0003).
- **API contract** — **Hurl** drives the actual HTTP endpoints.
- **Load** — **k6** smoke and throughput against a release build.

<!-- Note: Each level catches a different class of bug. Unit tests cover the logic; integration tests spin up a real Redis in a container so the riskiest dependency is not mocked away; Hurl validates the HTTP contract end to end; k6 gives a basic performance signal. All four run from `task`, and the first three run in CI. -->

---

# Continuous integration

Every push and PR runs through the **same `nix develop`** on GitHub Actions:

- **quality** — format check, clippy, secret scan
- **test** — build, unit, integration
- **container** — build, size gate, SBOM, CVE scan, publish to GHCR
- **docs** / **deploy-docs** — build, prose, links → GitHub Pages

<!-- Note: CI reuses the exact development environment, so a green pipeline means the project really works, not that it works in some bespoke CI image. The jobs mirror the local gate. The container job both enforces quality and, on main, publishes the image production pulls. Docs and slides deploy to Pages from the same run. -->

---

# Supply-chain security

- **Distroless, non-root** runtime — minimal surface.
- **SBOM** with **Syft**; image scanned by **Trivy** — build **fails on HIGH/CRITICAL**.
- **Secret scanning** with gitleaks, in CI and in the pre-commit hook.
- **Everything pinned** — `flake.lock`, `Cargo.lock`, and the deployed image by **digest**.

<!-- Note: Supply chain is a first-class concern here. We emit a software bill of materials, scan the image for known CVEs and block serious ones, and look for committed secrets both locally and in CI. Because crates, tools, and even the running image digest are pinned, what we test is exactly what we ship. -->

---

# GitOps delivery

- **Argo CD** continuously syncs the cluster to git — the repo is the source of truth.
- **Image Updater** watches GHCR and rolls out new images **by digest** — no manual deploy.
- **No secrets**: repo and image are public; write-back patches the app via the Kubernetes API.
- Runs on **k3s + Traefik**, HTTPS via Let's Encrypt, scaled by an **HPA**.

<!-- Note: Delivery is pull-based. I push to main, CI publishes a new image, and Image Updater notices the new digest and updates the Argo application, which rolls the deployment — I never run kubectl apply by hand. It needs no credentials because everything is public and the write-back uses in-cluster RBAC. The same manifests stand up TLS and autoscaling. -->

---

# Documentation as code

- **mdBook** handbook, published to **GitHub Pages** on every push to `main`.
- **ADRs** capture the _why_ behind each choice — Nix, distroless, SSE, Redis, Kubernetes.
- Prose linted by **Vale**, links by **lychee**, both in CI — docs can't silently rot.
- One source of truth: the site is generated from the repo's Markdown — including these slides.

<!-- Note: Docs live beside the code and ship through the same pipeline, so they cannot drift. The Architecture Decision Records explain the trade-offs, which is usually what a reviewer wants. Even this deck is Markdown in the repo, rendered by Marp and published to Pages next to the handbook — that is the "slides as code" idea. -->

---

# Code quality & guardrails

- **clippy** with `-D warnings` — a warning fails the build.
- **treefmt** formats every language from one command (rustfmt, prettier, shfmt, …).
- **Conventional Commits**, enforced by a `commit-msg` hook.
- **lefthook** hooks: pre-commit (format, lint, secret scan), pre-push (build, test).

<!-- Note: The guardrails make the good path the default. You cannot commit unformatted code or a non-conventional message, and you cannot push something that fails to build or breaks unit tests. clippy is an error gate, not advice. The result is a consistent codebase and history without relying on memory. -->

---

# What's next

- **Auth and rate-limiting** — today anyone can paint; add identity and abuse limits.
- **Durable history** — snapshot and replay the canvas beyond Redis' live state.
- **Observability** — metrics and tracing along the SSE and Redis path.
- Honest scope: these are deliberate omissions, not oversights.

<!-- Note: Being upfront about the limits is more convincing than pretending there are none. The app has no auth, so it is open-canvas by design; persistence is Redis-only; and there is no metrics stack yet. None of these are hard to add given the pipeline already in place — they simply were not the point of the exercise. -->

---

<!-- _class: lead -->

# Thank you

**Repo** · `github.com/Vallsp/PixelFlux`
**Docs** · `vallsp.github.io/PixelFlux`
**Slides** · `vallsp.github.io/PixelFlux/slides`

_Questions?_

<!-- Note: Wrap up — the takeaway is that a tiny app can still demonstrate a complete, production-grade software lifecycle. Point to the repo and the live docs and slides, then open for questions. Likely questions: why SSE over WebSockets, why Nix, and how the GitOps loop avoids secrets — each has an ADR or a slide behind it. -->
