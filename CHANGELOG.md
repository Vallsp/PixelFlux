# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Collaborative 64×64 pixel canvas (Rust / axum) with an embedded web UI.
- Real-time updates over Server-Sent Events, shared across instances with Redis
  pub/sub; in-memory fallback when no `REDIS_URL` is set.
- HTTP API: `/health`, `/info` (with serving instance), `/api/canvas`,
  `/api/pixel`, `/api/events`.
- Reproducible Nix dev shell and a distroless, non-root container (< 20 MB).
- Task runner (go-task) exposing build, test, lint, security, container, and
  deploy actions.
- Multi-level tests: unit, integration (Testcontainers + real Redis), API
  contract (Hurl + OpenAPI), and load (k6).
- Supply-chain security: gitleaks (secrets), Syft (SBOM), Trivy (CVE scan).
- Git hooks (lefthook) and Conventional Commits enforcement.
- CI (GitHub Actions): quality, tests, and container build + scan; image
  published to GHCR on push to `main`.
- Kubernetes deployment: Traefik load balancing, 3 replicas, HPA, and
  automatic HTTPS via Let's Encrypt; Argo CD Application for GitOps.
- Admin dashboard at `/admin` (enabled by `ADMIN_PASSWORD`): runtime-tunable
  limits (rate limit/window, registration delay, token TTL, presence timings),
  read-only maintenance mode, canvas reset, and live stats. Settings persist in
  Redis and propagate to every replica via a `config:events` pub/sub channel.
  Auth uses a constant-time password check and an `HttpOnly`, `SameSite=Strict`
  session cookie.
- Documentation: README with diagrams, CONTRIBUTING, per-directory READMEs,
  AGENTS.md, and Architecture Decision Records.

[Unreleased]: https://github.com/Vallsp/PixelFlux/commits/main
