// Input of `weebo-crd-docs` (@weebo/operator-docs), run by `task recu` /
// `npm run gen:crds`: what the CRD reference pages say beyond the CRDs
// themselves. Paths default to the expected tree (../deploy/crd →
// content/docs/crds + public/crd-schemas).
import { defineConfig } from "@weebo/operator-docs/crd-docs";

export default defineConfig({
  // `AngosLocalInstance` → example named `my-local-instance`.
  kindPrefix: "Angos",
  // Sidebar order follows the order you'd create things in, not the alphabet.
  order: ["AngosLocalInstance"],
  // The guide that explains each kind, linked from its reference page.
  guides: {
    AngosLocalInstance: ["Install via Helm", "/docs/guides/install"],
  },
  // A minimal, realistic `spec` per kind, checked against the schema (every
  // required field present, every key a real field): a renamed field fails
  // generation instead of shipping a stale example.
  examples: {
    AngosLocalInstance: {
      repositorySelector: { namespaces: { from: "Same" } },
    },
  },
  // The operator reference pages (content/docs/reference/), generated from
  // the Rust sources: reason codes (crates/domain/src/error.rs), Events,
  // metrics & health.
  reference: {
    metricsPrefix: "weebo_angos",
  },
});
