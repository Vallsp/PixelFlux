# GHCR pull-secret refresher

Production-grade credential for pulling the **private**
`ghcr.io/vallsp/pixelflux` image into the cluster.

Instead of a long-lived personal access token, a CronJob mints a short-lived
**GitHub App installation token** (~60 min) and writes it into the `ghcr-pull`
`dockerconfigjson` secret, then attaches that secret to the `pixelflux`
namespace's `default` ServiceAccount. The only persistent secret is the App
private key, scoped to `Packages: read` and owned by a machine identity — not a
person.

> Status: scaffold. It has not been run end to end here (no App credentials),
> so treat it as a reviewed template and verify the token actually pulls before
> relying on it — see step 6.

## One-time setup (repo owner)

Creating/installing the App needs **admin** on the repo/account, so the repo
owner (Vallsp) does this part.

1. **Create a GitHub App** (Settings → Developer settings → GitHub Apps → New):
   - Permission: **Repository → Packages: Read-only** (Metadata read is
     implied). No webhook needed.
   - Generate a **private key** (downloads a `.pem`) and note the **App ID**.
2. **Install** the App on the account and grant it this repository, then note
   the **Installation ID** from the install URL
   (`.../settings/installations/<ID>`).
3. If the package isn't auto-linked, grant the App access under the package's
   settings (`ghcr.io/vallsp/pixelflux` → Package settings → Manage access).

## Deploy (per cluster)

```bash
# 1. Supply the App credentials (never commit the filled-in version):
kubectl create secret generic github-app-credentials -n pixelflux \
  --from-literal=appId=<APP_ID> \
  --from-literal=installationId=<INSTALLATION_ID> \
  --from-file=privateKey=/path/to/app-private-key.pem

# 2. Install the refresher (RBAC + script + CronJob):
kubectl apply -f cluster/ghcr-pull/refresher.yaml

# 3. Populate the pull secret immediately (don't wait for the schedule):
kubectl -n pixelflux create job --from=cronjob/ghcr-pull-refresher ghcr-pull-bootstrap
kubectl -n pixelflux logs job/ghcr-pull-bootstrap
```

## Verify

```bash
kubectl -n pixelflux get secret ghcr-pull
kubectl -n pixelflux get serviceaccount default -o jsonpath='{.imagePullSecrets}'
kubectl -n pixelflux rollout restart deploy/pixelflux   # pods now pull from GHCR
```

If GHCR rejects the App installation token (some setups don't grant Apps
package pulls), fall back to a fine-grained PAT from a dedicated machine
account with read-only package access — the CronJob/secret wiring is identical;
only the token source changes.
