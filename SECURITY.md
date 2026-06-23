# Security Policy

## Supported versions

Pixelflux is pre-1.0; only the latest `main` is supported. Fixes land on `main`.

## Reporting a vulnerability

Please **do not** open a public issue for security problems.

- Preferred: open a private report via GitHub
  ([Security → Report a vulnerability](https://github.com/Vallsp/PixelFlux/security/advisories/new)).
- Or email the maintainers at **valentinlespine@gmail.com** with the details and,
  if possible, a minimal reproduction.

We aim to acknowledge a report within a few days and to fix confirmed issues on
`main` as soon as reasonably possible. Please give us a reasonable window to
release a fix before any public disclosure.

## What the project already does

Security is part of the pipeline, not an afterthought:

- **Secret scanning** — `gitleaks` runs in the pre-commit hook and in CI.
- **Container CVE scanning** — `trivy` scans the image in CI and fails the build
  on HIGH/CRITICAL findings (`task cve`).
- **SBOM** — a software bill of materials is generated with `syft` (`task sbom`).
- **Minimal attack surface** — the container is distroless (no shell, no package
  manager), runs as a non-root user with a read-only root filesystem.
