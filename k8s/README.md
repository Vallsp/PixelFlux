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
| `ingressroute.yaml`     | IngressRoute                                 | **HTTP** route (Traefik `web` entrypoint). Host is a placeholder. Part of the kustomization.                                                              |
| `ingressroute-tls.yaml` | Middleware, IngressRoute ×2                  | **HTTPS** route: HTTP→HTTPS redirect + TLS route with a Let's Encrypt cert. Applied separately by `task deploy:tls`.                                      |
| `traefik-acme.yaml`     | HelmChartConfig                              | Configures the k3s Traefik with a Let's Encrypt (ACME) resolver `le` using the HTTP-01 challenge, with a persistent cert store. Applied once per cluster. |
| `kustomization.yaml`    | Kustomization                                | Bundles `redis`, `pixelflux`, `hpa`, and the HTTP `ingressroute`.                                                                                         |

## Deploy flow (tasks)

The Taskfile wraps everything. On a single-node k3s host:

```bash
task deploy:k3s-install     # once: install k3s + Traefik
task deploy                 # build image -> import into k3s -> apply -k k8s/ -> rollout
```

Then expose it — pick **one** (both define the `pixelflux` route, last applied wins):

```bash
# HTTP
DOMAIN=your.domain.com task deploy:ingress

# or HTTPS with an automatic Let's Encrypt certificate (needs ports 80 + 443)
DOMAIN=your.domain.com ACME_EMAIL=you@domain.com task deploy:tls
```

`DOMAIN` is substituted into the `Host(...)` rule at apply time; the manifests
keep `pixelflux.example.com` as a placeholder so the domain lives only in the
command (or in the Argo CD Application, below).

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

After enabling HTTPS, use `task deploy:restart` (not `task deploy`) for code
changes, otherwise `apply -k` re-applies the HTTP `ingressroute` and overwrites
the TLS route.

## GitOps with Argo CD (optional)

`../argocd/application.yaml` is an Argo CD Application that syncs this `k8s/`
kustomization continuously (auto-sync, prune, self-heal) into the `pixelflux`
namespace. The per-cluster hostname is set there, in the kustomize patch.

Apply once into a cluster that already runs Argo CD:

```bash
kubectl apply -f ../argocd/application.yaml
```

Caveats with the current setup:

- It manages the **HTTP** route only — the TLS route, redirect, and ACME config
  are not part of the synced kustomization.
- `selfHeal` + `prune` will revert/remove manual changes, including a manually
  applied HTTPS route. Don't mix Argo CD with the `task deploy:tls` flow as-is.
- It deploys into the `pixelflux` namespace, so run `task deploy:down` first if
  you previously deployed manually, to avoid two copies.

## Requirements

- A cluster with the Traefik IngressRoute CRDs (default on k3s).
- For HTTPS: ports 80 and 443 reachable, and DNS pointing at the cluster.
