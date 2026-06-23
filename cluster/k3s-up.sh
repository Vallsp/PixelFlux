#!/usr/bin/env bash
# One-command GitOps bring-up on a local single-node k3s host:
#   k3s (Traefik bundled) + Argo CD + secrets (from .env) + the Application.
#
# Usage:  bash cluster/k3s-up.sh   (or: task k3s:up)
# Requires a .env file (copy .env.example). Idempotent; uses sudo for k3s.
#
# The k3s *direct* (non-GitOps) path is the existing `task deploy`. This adds
# the Argo CD GitOps path, mirroring `task k3d:up`.
set -euo pipefail

ARGOCD_REF="${ARGOCD_REF:-stable}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- load .env -------------------------------------------------------------
if [ ! -f "$REPO_ROOT/.env" ]; then
  echo "ERROR: $REPO_ROOT/.env not found — copy .env.example to .env and fill it in." >&2
  exit 1
fi
set -a
# shellcheck source=/dev/null
. "$REPO_ROOT/.env"
set +a
: "${GHCR_USER:?set GHCR_USER in .env}"
: "${GHCR_PAT:?set GHCR_PAT in .env}"
: "${DEPLOY_KEY_PATH:?set DEPLOY_KEY_PATH in .env}"
DEPLOY_KEY_PATH="${DEPLOY_KEY_PATH/#\~/$HOME}"
if [ ! -f "$DEPLOY_KEY_PATH" ]; then
  echo "ERROR: deploy key not found at $DEPLOY_KEY_PATH" >&2
  exit 1
fi

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

# --- 2. Argo CD ------------------------------------------------------------
echo "==> installing Argo CD"
"${KUBECTL[@]}" get namespace argocd >/dev/null 2>&1 || "${KUBECTL[@]}" create namespace argocd
"${KUBECTL[@]}" apply -n argocd \
  -f "https://raw.githubusercontent.com/argoproj/argo-cd/${ARGOCD_REF}/manifests/install.yaml"
"${KUBECTL[@]}" -n argocd rollout status deploy/argocd-server --timeout=300s

# --- 3. namespace + secrets (from .env) ------------------------------------
echo "==> creating namespace + secrets"
"${KUBECTL[@]}" get namespace pixelflux >/dev/null 2>&1 || "${KUBECTL[@]}" create namespace pixelflux
for _ in $(seq 1 30); do
  "${KUBECTL[@]}" -n pixelflux get serviceaccount default >/dev/null 2>&1 && break
  sleep 1
done

# Argo CD repository credential (SSH deploy key).
"${KUBECTL[@]}" create secret generic pixelflux-repo -n argocd \
  --from-literal=type=git \
  --from-literal=url=git@github.com:Vallsp/PixelFlux.git \
  --from-file=sshPrivateKey="$DEPLOY_KEY_PATH" \
  --dry-run=client -o yaml | "${KUBECTL[@]}" apply -f -
"${KUBECTL[@]}" -n argocd label secret pixelflux-repo \
  argocd.argoproj.io/secret-type=repository --overwrite

# GHCR pull credential, attached to the default ServiceAccount.
"${KUBECTL[@]}" create secret docker-registry ghcr-pull -n pixelflux \
  --docker-server=ghcr.io \
  --docker-username="$GHCR_USER" \
  --docker-password="$GHCR_PAT" \
  --dry-run=client -o yaml | "${KUBECTL[@]}" apply -f -
"${KUBECTL[@]}" -n pixelflux patch serviceaccount default \
  -p '{"imagePullSecrets":[{"name":"ghcr-pull"}]}'

# --- 4. Application --------------------------------------------------------
echo "==> applying the Argo CD Application"
"${KUBECTL[@]}" apply -f "$REPO_ROOT/argocd/application.yaml"
"${KUBECTL[@]}" -n argocd annotate application pixelflux \
  argocd.argoproj.io/refresh=hard --overwrite >/dev/null

# --- 5. wait for the rollout + report --------------------------------------
echo "==> waiting for Argo to create and roll out the Deployment"
for _ in $(seq 1 60); do
  "${KUBECTL[@]}" -n pixelflux get deploy pixelflux >/dev/null 2>&1 && break
  sleep 3
done
"${KUBECTL[@]}" -n pixelflux rollout status deploy/pixelflux --timeout=300s

host="$("${KUBECTL[@]}" -n pixelflux get ingressroute pixelflux \
  -o jsonpath='{.spec.routes[0].match}' 2>/dev/null |
  sed -E 's/.*Host\(`([^`]+)`\).*/\1/')"
host="${host:-pixelflux.example.com}"
cat <<EOF

==> up. k3s Traefik serves on the host's :80, host-routed to $host:
    curl -H "Host: $host" http://localhost/
    # or add to /etc/hosts:  <host-ip> $host    then open http://$host
EOF
