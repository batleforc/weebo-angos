#!/usr/bin/env bash
# kind e2e (`task e2e`, .github/workflows/e2e.yml): deploys cert-manager and
# the operator chart into the current kube context, then the smoke — an
# AngosLocalInstance running a real Angos that must end Ready, a name the
# CRD refuses —, the configuration's lifecycle on that instance (a hot
# reload read in the pod's log, a configuration Angos refuses replaced by
# the accepted one, a storage change held until confirmed), and checks
# leader election.
#
# With a previously published chart, it first installs that release, creates
# the smoke CRs with it, then upgrades to this checkout's chart and checks
# they keep their state (skipped when no release exists yet).
#
# Env: IMAGE (operator image already loaded in the cluster, default
# weebo-angos-operator:e2e), CERT_MANAGER_VERSION (default: the pinned
# release below; its manifest is checksum-verified only for the pinned one),
# UPGRADE_FROM (previous chart version to upgrade from: empty = the latest
# published one, `none` = skip the upgrade test).
# -E: functions inherit the ERR trap that dumps diagnostics.
set -Eeuo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
CHART="$HERE/../charts/weebo-angos"
IMAGE="${IMAGE:-weebo-angos-operator:e2e}"
# Pinned: bump both together (sha256 of the release's cert-manager.yaml).
CERT_MANAGER_PINNED=v1.21.2
CERT_MANAGER_SHA256=e03b668ec8675214af6b0a671699d088f2601fa3878e0dbe1b41d3feafd1879f
CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION:-$CERT_MANAGER_PINNED}"
CHART_OCI=oci://ghcr.io/batleforc/charts/weebo-angos
UPGRADE_FROM="${UPGRADE_FROM:-}"
OPERATOR_NS=weebo-angos-system
RELEASE=weebo-angos
# The chart's default Lease name (leaderElection.leaseName).
LEASE=weebo-angos-operator-leader
WAIT=180s

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
fail() {
  printf '\033[31mFAIL: %s\033[0m\n' "$*" >&2
  dump
  exit 1
}

# Diagnostics for whatever failed: operator logs, CR conditions, events.
dump() {
  set +e
  step "diagnostics"
  kubectl -n "$OPERATOR_NS" get pods -o wide
  kubectl -n "$OPERATOR_NS" logs -l app.kubernetes.io/name=weebo-angos --tail=200 --prefix --all-containers
  kubectl -n team-a get angoslocalinstances,angosrepositories,angosidentities,angosmaintenances -o yaml
  kubectl -n team-a get jobs,pods -o wide
  kubectl -n team-a logs deploy/registry --timestamps --tail=400 | grep -v 'readyz\|healthz' | tail -40
  kubectl -n team-a get secret registry-config -o jsonpath='{.data.config\.toml}' | base64 -d | grep -A3 'auth.identity'
  kubectl -n team-a get secret ci-angos-password -o jsonpath='{.data.version}' | base64 -d; echo
  kubectl -n weebo-angos-system logs -l app.kubernetes.io/name=weebo-angos --tail=400 | grep -i 'identit\|ci\b' | tail -20
  kubectl get events -A --sort-by=.lastTimestamp | tail -n 50
}
# dump turns errexit off: leave right after it.
trap 'dump; exit 1' ERR

ensure_ns() { kubectl create namespace "$1" --dry-run=client -o yaml | kubectl apply -f - >/dev/null; }

# Waits until `Ready` of instance $1 has reason $2.
reason_is() {
  local name="$1" want="$2" got=""
  for _ in $(seq 1 90); do
    got="$(kubectl -n team-a get angoslocalinstance "$name" \
      -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}' 2>/dev/null || true)"
    [ "$got" = "$want" ] && return 0
    sleep 2
  done
  fail "$name: Ready reason is ${got:-<none>}, expected $want"
}

smoke_state() {
  reason_is registry Reconciled
}

# Retried while a webhook of the release comes up.
apply_smoke() {
  for _ in $(seq 1 30); do
    kubectl apply -f "$HERE/smoke.yaml" && return 0
    sleep 2
  done
  fail "the smoke CRs were never applied"
}

step "cert-manager ($CERT_MANAGER_VERSION)"
manifest="$(mktemp)"
curl -fsSLo "$manifest" \
  "https://github.com/cert-manager/cert-manager/releases/download/$CERT_MANAGER_VERSION/cert-manager.yaml"
if [ "$CERT_MANAGER_VERSION" = "$CERT_MANAGER_PINNED" ]; then
  echo "$CERT_MANAGER_SHA256  $manifest" | sha256sum -c - >/dev/null || fail "cert-manager.yaml checksum mismatch"
fi
kubectl apply -f "$manifest"
kubectl -n cert-manager wait --for=condition=Available --timeout="$WAIT" deploy --all

# ── Upgrade test, part 1: the previous release ────────────────────────────
head_version="$(awk '/^version:/ {print $2}' "$CHART/Chart.yaml")"
previous=""
if [ "$UPGRADE_FROM" != none ]; then
  if [ -n "$UPGRADE_FROM" ]; then
    previous="$UPGRADE_FROM"
  else
    # --devel: the alphas are prereleases, which Helm skips otherwise.
    previous="$(helm show chart "$CHART_OCI" --devel 2>/dev/null | awk '/^version:/ {print $2}' || true)"
  fi
  if [ -z "$previous" ]; then
    echo "no published chart at $CHART_OCI: upgrade test skipped"
  elif [ "$previous" = "$head_version" ] \
    && [ "$(git -C "$HERE" describe --exact-match --tags HEAD 2>/dev/null || true)" = "v$previous" ]; then
    # This checkout is that release: nothing to upgrade from.
    echo "this checkout is the published release $previous: upgrade test skipped"
    previous=""
  fi
fi

if [ -n "$previous" ]; then
  step "upgrade test: install the published chart $previous"
  helm install "$RELEASE" "$CHART_OCI" --version "$previous" \
    --namespace "$OPERATOR_NS" --create-namespace \
    --set extraEnv[0].name=RUST_LOG --set extraEnv[0].value=info \
    --wait --timeout 5m
  ensure_ns team-a
  apply_smoke
  smoke_state
  step "upgrade test: upgrade $previous -> $head_version (this checkout)"
fi

step "operator chart ($IMAGE)"
# The kit's library chart, as Chart.lock pins it.
helm dependency build --skip-refresh "$CHART"
helm upgrade --install "$RELEASE" "$CHART" \
  --namespace "$OPERATOR_NS" --create-namespace \
  --set image.repository="${IMAGE%:*}" --set image.tag="${IMAGE##*:}" \
  --set image.pullPolicy=Never \
  --set extraEnv[0].name=RUST_LOG --set extraEnv[0].value=info \
  --wait --timeout 5m
kubectl -n "$OPERATOR_NS" rollout status "deploy/$RELEASE" --timeout="$WAIT"

if [ -n "$previous" ]; then
  step "upgrade test: CRs created by $previous keep their state"
  kubectl -n team-a patch angoslocalinstance registry --subresource=status --type=merge \
    -p '{"status":{"conditions":[]}}'
  kubectl -n team-a annotate angoslocalinstance registry e2e/upgraded="$(date +%s)" --overwrite
  smoke_state
fi

step "smoke: an instance ends Ready"
ensure_ns team-a
apply_smoke
smoke_state

# `jsonpath` of the instance.
field() { kubectl -n team-a get angoslocalinstance registry -o jsonpath="$1"; }
pod_name() {
  kubectl -n team-a get pods -l app.kubernetes.io/instance=registry \
    --field-selector=status.phase=Running -o jsonpath='{.items[0].metadata.name}'
}
# Waits until the hot layer desired is the one every pod confirmed.
applied() {
  for _ in $(seq 1 120); do
    hash="$(field '{.status.configHash}')"
    [ -n "$hash" ] && [ "$hash" = "$(field '{.status.appliedConfigHash}')" ] && return 0
    sleep 2
  done
  fail "configuration $(field '{.status.configHash}') never confirmed: $(field '{.status.conditions}')"
}

step "a real Angos: its first configuration, its probes through the access policy"
first_pod="$(pod_name)"
[ -n "$first_pod" ] || fail "no Angos pod"
[ "$(field '{.status.version}')" = 1.12.3 ] || fail "status.version: $(field '{.status.version}')"
boot="$(field '{.status.bootHash}')"

step "a hot change: reloaded in place, confirmed from the pod's log"
kubectl -n team-a patch angoslocalinstance registry --type merge -p '{"spec":{"global":{"gcGraceSecs":900}}}'
applied
reason_is registry Reconciled
[ "$(pod_name)" = "$first_pod" ] || fail "a hot change replaced the pod"
[ "$(field '{.status.bootHash}')" = "$boot" ] || fail "a hot change rolled the startup layer"
kubectl -n team-a get events --field-selector reason=ConfigReloaded -o name | grep -q . \
  || fail "no ConfigReloaded event"

step "a configuration Angos refuses: the accepted one goes back"
accepted="$(kubectl -n team-a get secret registry-config-applied -o jsonpath='{.data.config\.toml}')"
# Not rendered by a typed field, so checked by Angos only: zero is refused.
kubectl -n team-a patch angoslocalinstance registry --type merge \
  -p '{"spec":{"extraConfig":{"global":{"max_concurrent_cache_jobs":0}}}}'
reason_is registry ConfigRejected
message="$(field '{.status.rejectionMessage}')"
printf '%s' "$message" | grep -q nonzero || fail "rejection message: $message"
[ "$(kubectl -n team-a get secret registry-config -o jsonpath='{.data.config\.toml}')" = "$accepted" ] \
  || fail "the refused configuration was left in registry-config"
# A pod that restarts now starts, on the accepted configuration.
kubectl -n team-a delete pod "$first_pod" --timeout="$WAIT"
kubectl -n team-a wait pods -l app.kubernetes.io/instance=registry --for=condition=Ready --timeout="$WAIT"
reason_is registry ConfigRejected
kubectl -n team-a patch angoslocalinstance registry --type json -p '[{"op":"remove","path":"/spec/extraConfig"}]'
applied
reason_is registry Reconciled

step "a storage change waits for its confirmation"
kubectl -n team-a patch angoslocalinstance registry --type merge -p '{"spec":{"storage":{"fs":{"rootDir":"/data2"}}}}'
for _ in $(seq 1 60); do
  pending="$(field '{.status.pendingBootHash}')"
  [ -n "$pending" ] && break
  sleep 2
done
[ -n "$pending" ] || fail "no pendingBootHash"
field '{.status.conditions[?(@.type=="StorageChangePending")].message}' | grep -q '/data2' \
  || fail "no StorageChangePending advisory"
[ "$(field '{.status.bootHash}')" = "$boot" ] || fail "the storage moved without a confirmation"
kubectl -n team-a annotate angoslocalinstance registry angos.weebo.io/confirm-storage="$pending" --overwrite
for _ in $(seq 1 90); do
  [ "$(field '{.status.bootHash}')" = "$pending" ] && break
  sleep 2
done
[ "$(field '{.status.bootHash}')" = "$pending" ] || fail "the confirmed storage change never rolled"
reason_is registry Reconciled
[ -z "$(field '{.status.pendingBootHash}')" ] || fail "pendingBootHash left after the roll"

# `kind` `name`'s Ready reason, waited for (3 minutes).
cr_reason_is() {
  for _ in $(seq 1 90); do
    got="$(kubectl -n team-a get "$1" "$2" \
      -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}' 2>/dev/null || true)"
    [ "$got" = "$3" ] && return 0
    sleep 2
  done
  fail "$1 $2: Ready reason is ${got:-<none>}, expected $3"
}
# An HTTP request to the registry from inside the cluster: its status code.
registry() {
  kubectl -n team-a exec e2e-probe -- curl -s -o /dev/null -w '%{http_code}' "$@"
}

step "repositories: a pull-through of ghcr.io, a scratch space, rules on Angos's engine"
kubectl apply -f - <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosRepository
metadata:
  name: ghcr
  namespace: team-a
spec:
  prefix: ghcr
  access:
    pull: [anonymous]
  upstreams:
    - url: https://ghcr.io
---
apiVersion: angos.weebo.io/v1alpha1
kind: AngosRepository
metadata:
  name: scratch
  namespace: team-a
spec:
  prefix: scratch
  access:
    pull: [anonymous]
    push: [anonymous]
YAML
cr_reason_is angosrepository ghcr Reconciled
cr_reason_is angosrepository scratch Reconciled
kubectl -n team-a run e2e-probe --restart=Never --image=docker.io/curlimages/curl:8.17.0 \
  --command -- sleep 3600 >/dev/null
kubectl -n team-a wait pod/e2e-probe --for=condition=Ready --timeout="$WAIT" >/dev/null
base=http://registry.team-a.svc:8000/v2
# Pulled through from ghcr.io, anonymously.
code="$(registry -H 'Accept: application/vnd.oci.image.index.v1+json' \
  "$base/ghcr/project-angos/angos/manifests/1.12.3")"
[ "$code" = 200 ] || fail "pull-through of ghcr.io answered $code"
# A pull-through refuses every push.
code="$(registry -X POST "$base/ghcr/e2e/blobs/uploads/")"
case "$code" in 202) fail "a push to the pull-through was accepted" ;; esac
# The scratch space takes one; a namespace no repository declares doesn't.
code="$(registry -X POST "$base/scratch/e2e/blobs/uploads/")"
[ "$code" = 202 ] || fail "a push to scratch answered $code"
code="$(registry -X POST "$base/undeclared/e2e/blobs/uploads/")"
case "$code" in 202) fail "a push to an undeclared namespace was accepted" ;; esac
# What would crash Angos's CEL parser, and an overlapping prefix: refused.
if kubectl apply -f - 2>/dev/null <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosRepository
metadata:
  name: broken
  namespace: team-a
spec:
  prefix: broken
  access:
    rules: ["request.action =="]
YAML
then fail "a repository with a broken CEL rule was admitted"; fi
if kubectl apply -f - 2>/dev/null <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosRepository
metadata:
  name: overlap
  namespace: team-a
spec:
  prefix: scratch/sub
YAML
then fail "an overlapping prefix was admitted"; fi

step "an S3 store, TLS and the cluster's tokens on a second instance"
kubectl apply -f "$HERE/secure.yaml"
kubectl -n team-a wait --for=condition=complete job/rustfs-bucket --timeout="$WAIT"
kubectl -n team-a wait --for=condition=Ready certificate/secure-tls --timeout="$WAIT"
for _ in $(seq 1 90); do
  got="$(kubectl -n team-a get angoslocalinstance secure \
    -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}' 2>/dev/null || true)"
  [ "$got" = Reconciled ] && break
  sleep 2
done
[ "$got" = Reconciled ] || fail "secure: Ready reason is ${got:-<none>}, expected Reconciled"
# Served over TLS (self-signed: not verified here).
# (`kubectl run -i --rm` may attach after a short command exited and print
# nothing: the long-lived probe pod is used.)
code="$(registry -k https://secure.team-a.svc:8000/readyz)"
[ "$code" = 200 ] || fail "secure: /readyz over TLS answered $code"
# The generated token key and the cluster's issuer reached the file.
kubectl -n team-a get secret secure-token-key -o name >/dev/null || fail "secure: no generated token key"
kubectl -n team-a get secret secure-config -o jsonpath='{.data.config\.toml}' | base64 -d \
  | grep -q 'bearer_token_file = "/var/run/secrets/angos/kube/token"' \
  || fail "secure: auth.kubernetes not rendered"

step "an identity: its password works, and a rotation replaces it"
kubectl apply -f - <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosIdentity
metadata:
  name: ci
  namespace: team-a
spec:
  instanceRef: { name: registry }
---
apiVersion: angos.weebo.io/v1alpha1
kind: AngosRepository
metadata:
  name: ci
  namespace: team-a
spec:
  instanceRef: { name: registry }
  prefix: ci
  access:
    push: ["identity:ci"]
YAML
cr_reason_is angosidentity ci Reconciled
cr_reason_is angosrepository ci Reconciled
password="$(kubectl -n team-a get secret ci-credentials -o jsonpath='{.data.password}' | base64 -d)"
[ -n "$password" ] || fail "ci: no password in ci-credentials"
code="$(registry -u "ci:$password" -X POST "$base/ci/e2e/blobs/uploads/")"
[ "$code" = 202 ] || fail "ci: a push with its password answered $code"
code="$(registry -X POST "$base/ci/e2e/blobs/uploads/")"
case "$code" in 202) fail "an anonymous push to ci was accepted" ;; esac
kubectl -n team-a annotate angosidentity ci angos.weebo.io/rotate="$(date +%s)" --overwrite
for _ in $(seq 1 90); do
  [ "$(kubectl -n team-a get angosidentity ci -o jsonpath='{.status.passwordVersion}')" = 2 ] && break
  sleep 2
done
[ "$(kubectl -n team-a get angosidentity ci -o jsonpath='{.status.passwordVersion}')" = 2 ] \
  || fail "ci: the rotation never reached the targets"
rotated="$(kubectl -n team-a get secret ci-credentials -o jsonpath='{.data.password}' | base64 -d)"
[ "$rotated" != "$password" ] || fail "ci: the password didn't change"
code="$(registry -u "ci:$rotated" -X POST "$base/ci/e2e/blobs/uploads/")"
[ "$code" = 202 ] || fail "ci: a push with the new password answered $code"
code="$(registry -u "ci:$password" -X POST "$base/ci/e2e/blobs/uploads/")"
case "$code" in 202) fail "ci: the old password still works" ;; esac

step "a prune: skipped while a repository could lose its data, then a real dry run"
kubectl -n team-a patch angoslocalinstance secure --type merge -p '{"spec":{"pruneLock":{"windowHours":0}}}'
kubectl apply -f - <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosRepository
metadata:
  name: lost
  namespace: team-a
spec:
  instanceRef: { name: secure }
  prefix: lost
  upstreams:
    - url: https://ghcr.io
      credentialsSecretRef: { name: missing }
---
apiVersion: angos.weebo.io/v1alpha1
kind: AngosMaintenance
metadata:
  name: prune
  namespace: team-a
spec:
  instanceRef: { name: secure }
  task: Prune
YAML
cr_reason_is angosrepository lost SecretMissing
for _ in $(seq 1 60); do
  skipped="$(kubectl -n team-a get angosmaintenance prune \
    -o jsonpath='{.status.conditions[?(@.type=="MaintenanceSkipped")].message}')"
  [ -n "$skipped" ] && break
  sleep 2
done
printf '%s' "$skipped" | grep -q 'team-a/lost' || fail "prune: not skipped for team-a/lost: $skipped"
[ -z "$(kubectl -n team-a get angosmaintenance prune -o jsonpath='{.status.lastRun}')" ] \
  || fail "prune: a run started despite the lock"
# Its deletion needs the consent, then the lock lifts.
kubectl -n team-a delete angosrepository lost --wait=false
cr_reason_is angosrepository lost DeletionNotAllowed
kubectl -n team-a annotate angosrepository lost angos.weebo.io/allow-data-deletion=true
kubectl -n team-a wait --for=delete angosrepository/lost --timeout="$WAIT"
kubectl -n team-a annotate angosmaintenance prune angos.weebo.io/run-now="$(date +%s)" --overwrite
for _ in $(seq 1 120); do
  state="$(kubectl -n team-a get angosmaintenance prune -o jsonpath='{.status.lastRun.state}')"
  [ "$state" = Succeeded ] && break
  case "$state" in Failed|TimedOut|Interrupted)
    fail "prune: $state: $(kubectl -n team-a get angosmaintenance prune -o jsonpath='{.status.lastRun.message}')" ;;
  esac
  sleep 2
done
[ "$state" = Succeeded ] || fail "prune: last run ${state:-<none>}"
[ -n "$(kubectl -n team-a get angosmaintenance prune -o jsonpath='{.status.lastDryRun.time}')" ] \
  || fail "prune: no dry-run report"

step "a worker, then two server pods once Angos probed the store"
kubectl -n team-a patch angoslocalinstance secure --type merge \
  -p '{"spec":{"replicas":2,"worker":{"enabled":true}}}'
for _ in $(seq 1 150); do
  writes="$(kubectl -n team-a get angoslocalinstance secure -o jsonpath='{.status.conditionalWrites.atomic}')"
  got="$(kubectl -n team-a get angoslocalinstance secure \
    -o jsonpath='{.status.conditions[?(@.type=="Ready")].reason}')"
  [ -n "$writes" ] && [ "$got" = Reconciled ] && break
  sleep 2
done
[ -n "$writes" ] || fail "secure: Angos's probe of the store was never read"
[ "$got" = Reconciled ] || fail "secure: Ready reason is ${got:-<none>} with the worker"
echo "RustFS conditional writes atomic: $writes"
kubectl -n team-a rollout status deployment/secure-worker --timeout="$WAIT"
[ "$(kubectl -n team-a get angoslocalinstance secure -o jsonpath='{.status.availableWorkers}')" -ge 1 ] \
  || fail "secure: no worker available"
if [ "$writes" = true ]; then
  for _ in $(seq 1 90); do
    [ "$(kubectl -n team-a get deployment secure -o jsonpath='{.status.readyReplicas}')" = 2 ] && break
    sleep 2
  done
  [ "$(kubectl -n team-a get deployment secure -o jsonpath='{.status.readyReplicas}')" = 2 ] \
    || fail "secure: not scaled to 2 on an atomic store"
  kubectl -n team-a get poddisruptionbudget secure >/dev/null || fail "secure: no PodDisruptionBudget"
else
  kubectl -n team-a get angoslocalinstance secure \
    -o jsonpath='{.status.conditions[?(@.type=="ReplicasHeld")].status}' | grep -q True \
    || fail "secure: advisory claims but the replicas aren't held"
fi
# The Service reaches the server only.
code="$(registry -k https://secure.team-a.svc:8000/readyz)"
[ "$code" = 200 ] || fail "secure: /readyz answered $code with the worker"

step "a scanner and an event webhook: rendered, and accepted by every pod"
kubectl -n team-a create secret generic hooks --from-literal=token=e2e-token \
  --dry-run=client -o yaml | kubectl label --local -f - angos.weebo.io/credential=true -o yaml \
  | kubectl apply -f -
applied_before="$(kubectl -n team-a get angoslocalinstance secure -o jsonpath='{.status.appliedConfigHash}')"
kubectl -n team-a patch angoslocalinstance secure --type merge -p '{"spec":{
  "scan":{"url":"http://scanner.team-a.svc:8766","tokenSecretRef":{"name":"hooks","key":"token"},"default":"Skip"},
  "eventWebhooks":[{"name":"audit","url":"http://audit.team-a.svc/events","policy":"Async",
    "tokenSecretRef":{"name":"hooks","key":"token"},"events":["manifest.push"],"default":true}]}}'
for _ in $(seq 1 120); do
  hash="$(kubectl -n team-a get angoslocalinstance secure -o jsonpath='{.status.appliedConfigHash}')"
  want="$(kubectl -n team-a get angoslocalinstance secure -o jsonpath='{.status.configHash}')"
  [ -n "$hash" ] && [ "$hash" != "$applied_before" ] && [ "$hash" = "$want" ] && break
  sleep 2
done
[ "$hash" = "$want" ] && [ "$hash" != "$applied_before" ] \
  || fail "secure: the scanner and the webhook were never confirmed ($(kubectl -n team-a get angoslocalinstance secure -o jsonpath='{.status.conditions}'))"
hot="$(kubectl -n team-a get secret secure-config -o jsonpath='{.data.config\.toml}' | base64 -d)"
printf '%s' "$hot" | grep -q '^\[global.scan\]' || fail "secure: no [global.scan]"
printf '%s' "$hot" | grep -q '^\[event_webhook.audit\]' || fail "secure: no [event_webhook.audit]"

step "the webhook refuses what the operator wouldn't run"
if kubectl apply -f - 2>/dev/null <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosLocalInstance
metadata:
  name: refused
  namespace: team-a
spec:
  image: { repository: docker.io/library/busybox }
  storage:
    fs: {}
YAML
then fail "an instance on a foreign image was admitted"; fi

step "the CRD refuses a name that can't name a Service"
if kubectl apply -f - 2>/dev/null <<'YAML'
apiVersion: angos.weebo.io/v1alpha1
kind: AngosLocalInstance
metadata:
  name: not.a.label
  namespace: team-a
spec:
  storage:
    fs: {}
YAML
then fail "an instance named not.a.label was admitted"; fi

step "leader election"
holder="$(kubectl -n "$OPERATOR_NS" get lease "$LEASE" -o jsonpath='{.spec.holderIdentity}')"
[ -n "$holder" ] || fail "nobody holds the lease $LEASE"
kubectl -n "$OPERATOR_NS" delete pod "$holder" --wait=false
for _ in $(seq 1 60); do
  now="$(kubectl -n "$OPERATOR_NS" get lease "$LEASE" -o jsonpath='{.spec.holderIdentity}')"
  if [ -n "$now" ] && [ "$now" != "$holder" ]; then break; fi
  sleep 2
done
[ "$now" != "$holder" ] || fail "the lease never moved off $holder"
# The new leader reconciles: a touched instance gets its state again.
kubectl -n team-a patch angoslocalinstance registry --subresource=status --type=merge \
  -p '{"status":{"conditions":[]}}'
kubectl -n team-a annotate angoslocalinstance registry e2e/touched="$(date +%s)" --overwrite
smoke_state

step "e2e passed"
