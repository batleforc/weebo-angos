// Input of `weebo-crd-docs` (@weebo/operator-docs), run by `task recu` /
// `npm run gen:crds`: what the CRD reference pages say beyond the CRDs
// themselves. Paths default to the expected tree (../deploy/crd →
// content/docs/crds + public/crd-schemas).
import { defineConfig } from "@weebo/operator-docs/crd-docs";

export default defineConfig({
  // `AngosLocalInstance` → example named `my-local-instance`.
  kindPrefix: "Angos",
  // Sidebar order follows the order you'd create things in, not the alphabet.
  order: ["AngosLocalInstance", "AngosRepository", "AngosIdentity", "AngosMaintenance"],
  // The guide that explains each kind, linked from its reference page.
  guides: {
    AngosLocalInstance: ["Install via Helm", "/docs/guides/install"],
    AngosRepository: ["Install via Helm", "/docs/guides/install"],
    AngosIdentity: ["Install via Helm", "/docs/guides/install"],
    AngosMaintenance: ["Install via Helm", "/docs/guides/install"],
  },
  // A minimal, realistic `spec` per kind, checked against the schema (every
  // required field present, every key a real field): a renamed field fails
  // generation instead of shipping a stale example.
  examples: {
    AngosLocalInstance: {
      storage: {
        s3: {
          endpoint: "https://s3.example.com",
          bucket: "registry",
          region: "us-east-1",
          credentialsSecretRef: { name: "registry-s3" },
        },
      },
      auth: {
        tokenService: { realm: "https://registry.example.com/token" },
      },
    },
    AngosRepository: {
      prefix: "dockerhub",
      access: { pull: ["authenticated"] },
      upstreams: [{ url: "https://registry-1.docker.io" }],
    },
    AngosIdentity: {
      registryHost: "registry.example.com",
      rotationDays: 90,
    },
    AngosMaintenance: {
      task: "Prune",
      schedule: "15 * * * *",
      params: { dryRun: true, uploadTimeout: "24h" },
    },
  },
  // The operator reference pages (content/docs/reference/), generated from
  // the Rust sources: reason codes (crates/domain/src/error.rs), Events,
  // metrics & health.
  reference: {
    metricsPrefix: "weebo_angos",
    extraMetrics: [
      {
        name: "weebo_angos_instance_ready",
        type: "gauge",
        labels: "`instance_namespace`, `instance_name`",
        description: "1 when the AngosLocalInstance is `Ready`, 0 otherwise; gone with the instance",
      },
      {
        name: "weebo_angos_config_reloads_total",
        type: "counter",
        labels: "`instance_namespace`, `instance_name`, `result` (`applied`, `rejected`, `unconfirmed`: the pods rolled instead)",
        description: "Hot layers the pods answered",
      },
      {
        name: "weebo_angos_rollouts_total",
        type: "counter",
        labels: "`instance_namespace`, `instance_name`, `cause` (`boot`, `image`, `storage`, `fallback`)",
        description: "Rollouts of an instance's pods: a startup layer, an Angos version, a confirmed storage change, a reload they didn't confirm",
      },
      {
        name: "weebo_angos_maintenance_runs_total",
        type: "counter",
        labels: "`task`, `result` (`success`, `failure`, `skipped`)",
        description: "Maintenance runs ended or skipped",
      },
    ],
  },
});
