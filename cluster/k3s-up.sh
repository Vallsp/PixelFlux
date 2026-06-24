#!/usr/bin/env bash
# One-command GitOps bring-up on a single-node k3s host:
#   k3s (Traefik bundled) + Argo CD + the pixelflux Application, in HTTPS.
#
# The repo and the GHCR image are PUBLIC, so no secrets are needed — Argo clones
# over anonymous HTTPS and the node pulls the public image. The only per-cluster
# settings (DOMAIN, ACME_EMAIL) come from cluster/config.env (or the environment).
#
# Usage:  cp cluster/config.env.example cluster/config.env   # then edit it
#         bash cluster/k3s-up.sh   (or: task k3s:up). Idempotent; uses sudo for k3s.
set -euo pipefail

ARGOCD_REF="${ARGOCD_REF:-stable}"
# Pinned: v1.x is a CRD rewrite with transitional docs; v0.18.0 is the last
# classic annotation-driven release (tokenless "argocd" write-back).
IMAGE_UPDATER_REF="${IMAGE_UPDATER_REF:-v0.18.0}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- config (DOMAIN, ACME_EMAIL): env vars win, else cluster/config.env ------
_D="${DOMAIN-}"
_E="${ACME_EMAIL-}"
# shellcheck source=/dev/null
[ -f "$REPO_ROOT/cluster/config.env" ] && . "$REPO_ROOT/cluster/config.env"
DOMAIN="${_D:-${DOMAIN-}}"
ACME_EMAIL="${_E:-${ACME_EMAIL-}}"
: "${DOMAIN:?set DOMAIN in cluster/config.env (copy config.env.example) or the environment}"
: "${ACME_EMAIL:?set ACME_EMAIL in cluster/config.env or the environment}"
echo "==> domain=$DOMAIN  acme-email=$ACME_EMAIL"

# --- 1. k3s ----------------------------------------------------------------
if command -v k3s >/dev/null 2>&1 || [ -x /usr/local/bin/k3s ]; then
  echo "==> k3s already installed"
else
  echo "==> installing k3s (bundles Traefik on :80/:443)"
  curl -sfL https://get.k3s.io | sh -
fi

# --- pick a kubectl that talks to k3s (kubeconfig is root-owned) -----------
if command -v kubectl >/dev/null 2>&1 && kubectl cluster-info >/dev/null 2>&1; then
  KUBECTL=(kubectl)
elif command -v kubectl >/dev/null 2>&1 && [ -r /etc/rancher/k3s/k3s.yaml ]; then
  export KUBECONFIG=/etc/rancher/k3s/k3s.yaml
  KUBECTL=(kubectl)
elif command -v k3s >/dev/null 2>&1; then
  KUBECTL=(sudo k3s kubectl)
else
  KUBECTL=(sudo /usr/local/bin/k3s kubectl)
fi

echo "==> waiting for the node to be Ready"
"${KUBECTL[@]}" wait --for=condition=Ready node --all --timeout=180s

# --- 2. ACME resolver for the cluster Traefik (email from config) ----------
# Cluster-level config, applied directly (not via Argo) so the email stays a
# config value rather than committed YAML.
echo "==> configuring Let's Encrypt (ACME) on the cluster Traefik"
sed "s/admin@example.com/$ACME_EMAIL/" "$REPO_ROOT/k8s/traefik-acme.yaml" |
  "${KUBECTL[@]}" apply -f -

# --- 3. Argo CD ------------------------------------------------------------
echo "==> installing Argo CD"
"${KUBECTL[@]}" get namespace argocd >/dev/null 2>&1 || "${KUBECTL[@]}" create namespace argocd
# --server-side: Argo's CRDs are too big for a client-side apply (the
# last-applied-configuration annotation would exceed 262144 bytes).
"${KUBECTL[@]}" apply -n argocd --server-side --force-conflicts \
  -f "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_REF}/manifests/install.yaml"
"${KUBECTL[@]}" -n argocd rollout status deploy/argocd-server --timeout=300s

# --- 3b. Argo CD Image Updater (auto-rolls out new :latest digests) --------
# Runs in the argocd namespace; its Role can patch Applications, so the
# "argocd" write-back needs no token. The GHCR package is public, so no
# registry secret either. See the annotations in argocd/application.yaml.
echo "==> installing Argo CD Image Updater ($IMAGE_UPDATER_REF)"
"${KUBECTL[@]}" apply -n argocd \
  -f "https://raw.githubusercontent.com/argoproj-labs/argocd-image-updater/${IMAGE_UPDATER_REF}/manifests/install.yaml"
"${KUBECTL[@]}" -n argocd rollout status deploy/argocd-image-updater --timeout=180s

# --- 4. Application (domain rendered into the host patches) -----------------
echo "==> applying the Argo CD Application (host=$DOMAIN)"
sed "s/pixelflux\.example\.com/$DOMAIN/g" "$REPO_ROOT/argocd/application.yaml" |
  "${KUBECTL[@]}" apply -f -
"${KUBECTL[@]}" -n argocd annotate application pixelflux \
  argocd.argoproj.io/refresh=hard --overwrite >/dev/null

# --- 5. wait for the rollout + report --------------------------------------
echo "==> waiting for Argo to create and roll out the Deployment"
for _ in $(seq 1 60); do
  "${KUBECTL[@]}" -n pixelflux get deploy pixelflux >/dev/null 2>&1 && break
  sleep 3
done
"${KUBECTL[@]}" -n pixelflux rollout status deploy/pixelflux --timeout=300s

cat <<EOF

==> up. k3s Traefik serves on the host's :80/:443, host-routed to $DOMAIN:
    open https://$DOMAIN   (Let's Encrypt cert issues on the first request, ~1 min)
EOF
