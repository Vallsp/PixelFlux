# Kubernetes manifests

Deploys Pixelflux on Kubernetes behind Traefik: **3 app replicas** load-balanced
by a Service, a **Redis** for the shared canvas + real-time pub/sub, autoscaling,
and a Traefik route (HTTP or HTTPS). Tested on single-node **k3s** (which bundles
Traefik), but works on any cluster with the Traefik CRDs.

## Files

| File                    | Kind(s)                                      | Purpose                                                                                                                                                   |
| ----------------------- | -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `redis.yaml`            | Deployment, Service                          | Redis for shared canvas state and pub/sub fan-out. Ephemeral (no PVC).                                                                                    |
| `pixelflux.yaml`        | Deployment, Service                          | The app: 3 replicas, non-root (uid 65532), read-only root FS, `/health` probes. Service exposes port 80 → 3000.                                           |
| `hpa.yaml`              | HorizontalPodAutoscaler, PodDisruptionBudget | Autoscale 3→10 at 70% CPU; keep ≥2 fronts available during disruptions.                                                                                   |
| `ingressroute-tls.yaml` | Middleware, IngressRoute ×2                  | **HTTPS** route: HTTP→HTTPS redirect + TLS route with a Let's Encrypt cert. Host is a placeholder. **In the kustomization.**                              |
| `traefik-acme.yaml`     | HelmChartConfig                              | Configures the k3s Traefik with a Let's Encrypt (ACME) resolver `le` (HTTP-01, persistent store). Email comes from `config.env`; applied once by the bring-up script — **not** in the kustomization. |
| `ingressroute.yaml`     | IngressRoute                                 | Plain **HTTP** route (Traefik `web` entrypoint). Not in the kustomization — kept for an HTTP-only setup (`task deploy:ingress`).                          |
| `kustomization.yaml`    | Kustomization                                | Bundles `redis`, `pixelflux`, `hpa`, and the **HTTPS routing** (`ingressroute-tls`).                                                                     |

## Deploy flow (tasks)

The Taskfile wraps everything. On a single-node k3s host:

```bash
task deploy:k3s-install     # once: install k3s + Traefik
task deploy                 # build image -> import into k3s -> apply -k k8s/ -> rollout
```

`task deploy` applies the **HTTPS** bundle, but with the `pixelflux.example.com`
placeholder host. Set your real domain (and the ACME email) with:

```bash
DOMAIN=your.domain.com ACME_EMAIL=you@domain.com task deploy:tls
```

This substitutes `DOMAIN` into the `Host(...)` rules and the ACME email at apply
time; the manifests keep `pixelflux.example.com` as a placeholder so the domain
lives only in the command (or in the Argo CD Application, below). The certificate
is issued automatically on the first request (~1 min; needs ports 80 + 443). For
a plain **HTTP-only** setup instead, apply `ingressroute.yaml` with
`DOMAIN=your.domain.com task deploy:ingress` (it replaces the TLS `pixelflux`
route — last applied wins).

> The image is **not** pulled from a registry by default: `pixelflux.yaml` uses
> `image: pixelflux:latest` (`imagePullPolicy: IfNotPresent`), so it must already
> be on the node. `task deploy:image` builds it with Nix and imports it into
> k3s. For a remote/multi-node cluster, push to a registry (e.g. GHCR) and point
> `image:` there with `imagePullPolicy: Always`.

### Useful

```bash
task deploy:status     # pods, service, ingressroute, hpa
task deploy:logs       # tail logs from all replicas
task deploy:restart    # rebuild image + rollout (preserves the route)
task deploy:down       # remove the kustomized resources
```

For code changes after the first deploy, use `task deploy:restart` (rebuild +
rollout) so you don't re-run the full apply each time.

## GitOps with Argo CD (optional)

`../argocd/application.yaml` is an Argo CD Application that syncs this `k8s/`
kustomization continuously (auto-sync, prune, self-heal) into the `pixelflux`
namespace, with **HTTPS** (TLS route + redirect). The per-cluster hostname is set
there via the kustomize patches.

The repo and the GHCR image are public, so **no secrets are needed**. The only
settings are your domain and Let's Encrypt email, in `cluster/config.env` (not in
the manifests). One command installs Argo CD and applies everything:

```bash
cp ../cluster/config.env.example ../cluster/config.env   # edit DOMAIN + ACME_EMAIL
task k3s:up                                               # (local Docker: task k3d:up)
```

The bring-up script renders your `DOMAIN` into the Application's host patches and
applies `traefik-acme.yaml` (the cluster ACME resolver) with your `ACME_EMAIL`.

Caveats:

- `selfHeal` + `prune` will revert/remove anything you change by hand. Don't mix
  Argo CD with the manual `task deploy*` flow.
- It deploys into the `pixelflux` namespace, so run `task deploy:down` first if
  you previously deployed manually, to avoid two copies.
- Argo syncs manifests, not images — the GHCR image in the Application's
  `images:` override must be published and pullable.

## Requirements

- A cluster with the Traefik IngressRoute CRDs (default on k3s).
- For HTTPS: ports 80 and 443 reachable, and DNS pointing at the cluster.
