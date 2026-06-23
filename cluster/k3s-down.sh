#!/usr/bin/env bash
# Tear down the GitOps deployment on a single-node k3s host: delete the Argo CD
# Application. Its finalizer cascades, so Argo prunes everything it synced
# (the pixelflux namespace's workloads + routes). k3s and Argo CD themselves
# are left installed; remove them by hand if you want a full wipe.
#
# Usage:  bash cluster/k3s-down.sh   (or: task k3s:down)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Pick a kubectl that talks to k3s (kubeconfig is root-owned).
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

echo "==> deleting the Argo CD Application (cascades to its synced resources)"
"${KUBECTL[@]}" delete -f "$REPO_ROOT/argocd/application.yaml" --ignore-not-found
echo "==> done. k3s and Argo CD are still installed."
