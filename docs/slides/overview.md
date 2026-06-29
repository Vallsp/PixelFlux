---
marp: true
title: "PixelFlux — architecture & SDLC"
theme: pixelflux
paginate: true
---

<!-- _class: lead -->
<!-- _paginate: false -->

# PixelFlux

**A collaborative 64×64 pixel canvas — wrapped in a full production SDLC.**

Rust · axum · Redis pub/sub · SSE · Nix · Kubernetes · GitOps

---

# What it is

- A real-time multiplayer **pixel canvas**: pick a colour, paint a cell, everyone sees it live.
- **Rust / axum** HTTP server; **Redis pub/sub** fans edits out across instances.
- Live updates over **Server-Sent Events** — no polling, no WebSocket complexity.
- The point isn't the toy app — it's the **production pipeline** around it.

> Live at `vallsp.github.io/PixelFlux`; the running app shows its version and serving pod in the footer.

---

# It works — live

- Deployed on **k3s behind Traefik**, HTTPS via Let's Encrypt.
- **3 replicas**, load-balanced; the footer shows which pod answered (`served by …`).
- Paint in one tab → it appears instantly in another, **across pods** (Redis pub/sub).
- Introspection endpoints: `/health` and `/info` (name, version, instance).

---

# Build System · 5 pts

- **One command to a full toolbox:** `nix develop` — pinned, reproducible, everything included.
- **go-task** is the entrypoint: `task` lists every action (`build`, `run`, `test`, `deploy`, …).
- Distroless **image built by Nix** (`nix build .#container`) — deterministic, no Dockerfile drift.
- Locked inputs everywhere: `flake.lock` plus `Cargo.lock`.

```bash
nix develop          # the whole dev environment
task                 # list every task
```

---

# CI/CD & test environment · 5 pts

GitHub Actions on every push and PR — reproducible through the same `nix develop`:

- **quality** — format, lint, secret scan
- **test** — build, unit tests, **Testcontainers** integration (real Redis)
- **container** — build, size gate, SBOM, CVE scan, publish to GHCR
- **docs** / **deploy-docs** — mdBook build, prose, links → GitHub Pages

Four test levels: unit · integration (Testcontainers) · **API contract (Hurl)** · **load (k6)**.

---

# Supply-chain security · 5 pts

- **Distroless, non-root** image — no shell, no package manager.
- **Size gate:** the build fails if the image exceeds **20 MB**.
- **SBOM** with **Syft**; **CVE** scan with **Trivy** (fails on HIGH / CRITICAL).
- **gitleaks** secret scan, in CI and in the pre-commit hook.
- **GitOps** via Argo CD + **Image Updater** — new images roll out by **digest**, with **no secrets** (public repo and image).

---

# Documentation · 5 pts

- **mdBook** site, published to **GitHub Pages** on every push to `main`.
- **ADRs** (`docs/adr/`) record each key decision — Nix, distroless, SSE, and more.
- Prose checked with **Vale**, links with **lychee**, both in CI.
- One source of truth: the site is generated from the repo's Markdown, so it never drifts.

> These slides are part of it — authored as Markdown, rendered by **Marp**, published beside the docs.

---

# Code quality · 2 pts

- **clippy** with `-D warnings` — a warning is a failure.
- **treefmt** formats every language from one entrypoint (rustfmt, prettier, shfmt, …).
- **Conventional Commits**, enforced by a `commit-msg` hook (**lefthook**).
- The `pre-commit` hook formats, lints, and secret-scans staged files.

---

# How it fits together

```text
Browser ──HTTP──▶ axum ──┐
   ▲                      ├──▶ Redis   (canvas state + pub/sub)
   └────SSE◀── axum ◀─────┘
        every replica subscribes; edits fan out to all clients
```

- Stateless fronts → scale horizontally (HPA).
- Redis is the single source of canvas truth and the event bus.

---

<!-- _class: lead -->

# Thank you

**Repo:** `github.com/Vallsp/PixelFlux`
**Docs:** `vallsp.github.io/PixelFlux`
**Slides:** `vallsp.github.io/PixelFlux/slides`

Slides as code — Markdown plus Marp, built and published by CI.
