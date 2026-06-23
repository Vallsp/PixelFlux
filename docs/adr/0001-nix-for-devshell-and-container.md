# 0001. Use Nix for the dev shell and the container build

- **Status:** Accepted
- **Date:** 2026-06-18

## Context

A new contributor must be able to clone the repo and build it with nothing
installed but a couple of base tools, and the build must be reproducible across
machines and CI. We also need the toolchain used in CI to be identical to the
one used locally, to avoid "works on my machine" drift across the many tools the
project relies on (Rust, the task runner, linters, security scanners, k6, Hurl…).

## Decision

Use a **Nix flake** as the single source of truth for the environment:

- `nix develop` provides every tool at pinned versions (the dev shell).
- The container image is built by Nix (`dockerTools`) from the same flake.
- CI runs all steps inside `nix develop`, so CI and local use the same toolchain.

## Consequences

### Positive

- One command (`nix develop`) to a complete, reproducible environment.
- CI/local parity; no per-tool installation instructions to maintain.
- The flake lockfile pins everything, making builds reproducible.

### Trade-offs / negative

- Contributors must install Nix and enable flakes.
- Nix has a learning curve; first evaluation downloads a lot.

### Alternatives considered

- **mise / asdf** — simpler, but pins tool versions only, not a hermetic
  environment, and doesn't build the container.
- **Plain Dockerfile + docs** — easy to start, but drifts and isn't reproducible
  the way a flake lock is.
