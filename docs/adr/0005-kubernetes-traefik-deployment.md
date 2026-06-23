# 0005. Deploy on Kubernetes behind Traefik

- **Status:** Accepted
- **Date:** 2026-06-22

## Context

The app must run as several load-balanced replicas, scale with load, and be
reachable over HTTPS on a domain. We target a single, modest VPS for the demo but
want a setup that also works on a real cluster.

## Decision

Deploy on **Kubernetes**, using **k3s** on the VPS (it bundles Traefik). Traefik
is the ingress / load balancer that round-robins requests across the app pods via
a `Service`; a `HorizontalPodAutoscaler` adds replicas under load. HTTPS is issued
automatically by Traefik's ACME (Let's Encrypt) resolver. Manifests live in
`k8s/`, and an Argo CD `Application` is provided for GitOps delivery.

## Consequences

### Positive

- Real load balancing, autoscaling, self-healing, and rolling updates for free.
- Same manifests work on k3s and on a full cluster.
- Automatic, renewing TLS certificates.

### Trade-offs / negative

- Kubernetes adds operational complexity for what is a small app.
- The Argo CD path currently manages only the HTTP route; HTTPS is applied out of
  band (see `k8s/README.md`). Unifying them is future work.

### Alternatives considered

- **Docker Compose** — simpler, but no autoscaling/self-healing and weaker as a
  learning target for the workshop.
- **A single binary behind a reverse proxy** — fine for one instance, but doesn't
  demonstrate load balancing across replicas.
