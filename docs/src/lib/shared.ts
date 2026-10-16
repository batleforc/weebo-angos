import { defineSite } from '@weebo/operator-docs/site';

// What makes this site weebo-angos's; everything else comes from
// @weebo/operator-docs.
export const site = defineSite({
  name: 'weebo-angos',
  description:
    'Run Angos OCI registries — their configuration, repositories, identities and maintenance — as Kubernetes resources.',
  git: {
    user: 'batleforc',
    repo: 'weebo-angos',
    branch: 'main',
    contentDir: 'docs/content/docs',
  },
});
