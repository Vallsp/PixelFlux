# 0002. Ship a distroless, non-root, static container

- **Status:** Accepted
- **Date:** 2026-06-18

## Context

The deliverable must be a small, hardened container image with a near-zero CVE
count. A typical base image (Debian/Alpine) carries a shell, a package manager,
and libraries that add attack surface and CVEs we don't control.

## Decision

Build the binary as a **fully static `musl`** executable and package it in a
**distroless** image (built by Nix `dockerTools`) that contains only the binary
closure:

- No shell, no package manager, no libc layered on top.
- Runs as a **non-root** user (uid 65532) with a **read-only root filesystem**.
- A CI gate fails the build if the image exceeds 20 MB (`task container:size`),
  and Trivy scans it for CVEs.

## Consequences

### Positive

- Minimal attack surface and essentially nothing for CVE scanners to flag.
- Tiny image (low single-digit MB), fast to pull and deploy.
- Hardened runtime (non-root, read-only FS) by default.

### Trade-offs / negative

- No shell in the image, so `kubectl exec` debugging needs an ephemeral debug
  container instead.
- Static musl builds can be trickier for crates needing system libraries (not an
  issue for this app's dependencies).

### Alternatives considered

- **`gcr.io/distroless` base** — good, but a Nix-built image is reproducible and
  even more minimal.
- **Alpine** — small, but still ships a shell/package manager and musl as a
  system library.
