import { OperatorHome } from '@weebo/operator-docs/site';
import { site } from '@/lib/shared';

export default function HomePage() {
  return (
    <OperatorHome
      site={site}
      label="Kubernetes operator · Angos"
      caption="Run Angos, the OCI registry, from Kubernetes: its instances, then their repositories, identities and maintenance — its configuration rendered, checked and confirmed loaded."
      actions={[
        { href: '/docs/guides/install', label: 'Get started', primary: true },
        { href: '/docs', label: 'Read the docs' },
      ]}
      install={`helm install weebo-angos \\
  oci://ghcr.io/batleforc/charts/weebo-angos \\
  --namespace weebo-angos --create-namespace \\
  --set certManager.issuerRef.name=<issuer>`}
      example={{
        title: 'An instance',
        yaml: `apiVersion: angos.weebo.io/v1alpha1
kind: AngosLocalInstance
metadata:
  name: registry
  namespace: team-a
spec: {}`,
      }}
      features={[
        {
          title: 'Checked before loaded',
          body: 'Angos ignores unknown keys silently: the operator renders typed fields and checks them against the rules of the Angos version it targets.',
          href: '/docs',
          link: 'Overview',
        },
        {
          title: 'Secrets opt in',
          body: 'An instance runs a pod: it only reads or mounts Secrets labelled angos.weebo.io/credential, and only images of an allow-list.',
          href: '/docs/guides/install',
          link: 'Install',
        },
        {
          title: 'On the weebo kit',
          body: 'Same status, Events, metrics, chart and release pipeline as every weebo operator.',
          href: '/docs/reference/reason-codes',
          link: 'Reason codes',
        },
      ]}
    />
  );
}
