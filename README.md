# weebo-angos

Kubernetes operator running [Angos](https://github.com/project-angos/angos)
OCI registries, built on
[weebo-operator-kit](https://github.com/batleforc/weebo-operator-kit).
Angos has no administration API: the operator renders its configuration,
runs its workload, and reads what the pods did with it in their logs.

| Kind                 | Short | What it does                                                              |
| -------------------- | ----- | ------------------------------------------------------------------------- |
| `AngosLocalInstance` | `ali` | An Angos registry in a namespace: its workload and configuration          |
| `AngosRepository`    | `arepo` | A repository of an instance: its prefix, access, upstreams, downstreams |

Coming next (milestones M3–M5): `AngosIdentity` (`aid`),
`AngosMaintenance` (`amaint`), the worker and replicas.

Targets Angos **1.12.3** (`api::angos`): the configuration model
(`domain::config`), the rules Angos checks it with
(`domain::validation`, each citing Angos's code) and the startup keys are
those of that version; moving to another Angos release is a release of
the operator.

## Layout

Hexagonal, one crate per layer, same rules as every weebo operator:

| Crate               | Holds                                                                         |
| ------------------- | ----------------------------------------------------------------------------- |
| `domain`            | the reason codes, Angos's configuration model, its two layers and validation |
| `api`               | the CRD types (`#[derive(CustomResource)]`), the kit's shared field types      |
| `application`       | ports (the kit's `OwnedObjects`, `PodLogs`, `InstanceDirectory`), use cases    |
| `adapters-outbound` | the instance directory (the kit's `LocalInstances`), the workload settings    |
| `adapters-inbound`  | the controllers and the webhook router                                         |
| `operator`          | the binary: telemetry, leader election, webhook server, controllers           |
| `crdgen`            | writes `deploy/crd/` from `api`                                                |
| `testkit`           | envtest and fakes for the integration tests                                    |

`deploy/charts/weebo-angos` is a thin chart on the kit's library
chart: `templates/_params.tpl` holds what is the operator's own (group,
kinds, the namespaced rules of an instance's objects, `pods/log`, env),
`values.yaml` follows the kit's values contract plus `defaults` and
`managedInstances.allowedImages`. Creating an instance runs an Angos pod
with the namespace's Secrets labelled `angos.weebo.io/credential: "true"`.

## Tasks

```bash
task lint        # fmt --check, clippy -D warnings, actionlint, hadolint
task lint:helm   # helm lint + the kit's smoke renders
task test        # unit + contract tests
task test:all    # + envtest integration tests
task coverage    # cargo llvm-cov, fails under 85%
task audit       # cargo-deny + trivy + trivy config
task kit:sync TAG=vX.Y.Z  # move to another kit release (crates, tasks, CI, chart, cog.toml)
```

`lint`, `audit`, `coverage`, `doctor` and the chart tasks are the kit's
(`tasks/*.yaml`, included remotely and pinned by checksum); CI calls the
kit's reusable `checks.yml`.
