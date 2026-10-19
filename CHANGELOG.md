# Changelog
All notable changes to this project will be documented in this file. See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

- - -
## [v0.1.2](https://github.com/batleforc/weebo-angos/compare/c3f50f402a2dea79f0d0d277f994f5fee4129331..v0.1.2) - 2026-10-19
#### Miscellaneous Chores
- (**kit**) weebo-operator-kit v0.19.1, nested CRD fields in the reference (#14) - ([c3f50f4](https://github.com/batleforc/weebo-angos/commit/c3f50f402a2dea79f0d0d277f994f5fee4129331)) - Maxime, Claude Opus 5.5 (1M context)

- - -

## [v0.1.1](https://github.com/batleforc/weebo-angos/compare/aa2c02f8f06f1ae3bcc047fa0e2eea07f4351583..v0.1.1) - 2026-10-19
#### Miscellaneous Chores
- (**kit**) weebo-operator-kit v0.18.2, 4 tokio workers by default (#13) - ([aa2c02f](https://github.com/batleforc/weebo-angos/commit/aa2c02f8f06f1ae3bcc047fa0e2eea07f4351583)) - Maxime, Claude Opus 5.5 (1M context)

- - -

## [v0.1.0](https://github.com/batleforc/weebo-angos/compare/v0.1.0-alpha.2..v0.1.0) - 2026-10-16
#### Features
- the resource budget measured (task perf), and fewer wake-ups while waiting (#12) - ([efe6cf4](https://github.com/batleforc/weebo-angos/commit/efe6cf4a3447db5d6c692e75257931030eba8173)) - Maxime, Claude Opus 5.5
- weebo-angos-importer, weebo-si's chart as CRs, and the migration guide (M5) (#11) - ([1dbad7d](https://github.com/batleforc/weebo-angos/commit/1dbad7d6b4768e398b3ea38674a528ba887558a9)) - Maxime, Claude Opus 5.5
- the operator's metrics and alerts, scaling and monitoring guides (M4d) (#9) - ([eeed8dd](https://github.com/batleforc/weebo-angos/commit/eeed8dd20837d5d3b73a046c2f10f959676eabb1)) - Maxime, Claude Opus 5.5
- orchestrated Angos upgrades (M4c) (#8) - ([a12cdf6](https://github.com/batleforc/weebo-angos/commit/a12cdf6f322ff1e6f471f435fe5141a5f696dc9b)) - Maxime, Claude Opus 5.5
- scanner, event and authorization webhooks, client TLS (M4b) (#7) - ([60769e9](https://github.com/batleforc/weebo-angos/commit/60769e9bd3fc9d629ebadf4ff254c7541e784f4c)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- worker, replicas and Angos's conditional-write probe (M4a) (#6) - ([a9de8fd](https://github.com/batleforc/weebo-angos/commit/a9de8fd26001e2281eff6e99495d8055f171af21)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
#### Bug Fixes
- global.access.admin is granted in every repository too (#10) - ([7a0aafd](https://github.com/batleforc/weebo-angos/commit/7a0aafdd241eb10e1db658b3612f0edbbb2332f6)) - Maxime, Claude Opus 5.5

- - -

## [v0.1.0-alpha.2](https://github.com/batleforc/weebo-angos/compare/v0.1.0-alpha.1..v0.1.0-alpha.2) - 2026-10-16
#### Bug Fixes
- an identity's hash and version read at once (#5) - ([a34f119](https://github.com/batleforc/weebo-angos/commit/a34f119106c4593b9418af4688b98a812c93fbec)) - Maxime, Claude Opus 5.5
#### Miscellaneous Chores
- (**version**) v0.1.0-alpha.2 - ([f61e691](https://github.com/batleforc/weebo-angos/commit/f61e6914fdf67d7583a98487b5e8830a3afe2319)) - Max Batleforc

- - -

## [v0.1.0-alpha.1](https://github.com/batleforc/weebo-angos/compare/39871bdef90275e4fbd30da07f14cbc1b4864b7b..v0.1.0-alpha.1) - 2026-10-16
#### Features
- (**domain**) Angos 1.12.3 configuration model, layers and validation (M0) - ([4ea4485](https://github.com/batleforc/weebo-angos/commit/4ea4485fdb5d2c4ecde49af470e9e0a7ca109c19)) - Max Batleforc, Claude Opus 5.5
- AngosIdentity, AngosMaintenance and the prune lock (M3) (#4) - ([a826043](https://github.com/batleforc/weebo-angos/commit/a826043418917384e1c9656b88be940803dbb381)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- AngosRepository, access vocabulary, CEL on Angos's engine (M2) (#3) - ([d8deeb1](https://github.com/batleforc/weebo-angos/commit/d8deeb18108a2130e7571297e8d044414da6b0e9)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- instance admission, disruptive-update gate, live S3/TLS/kube-auth e2e (M1 leftovers) (#2) - ([abc48a0](https://github.com/batleforc/weebo-angos/commit/abc48a04a2754fb37ffa64efd795dca19620c350)) - Maxime, Claude Opus 5.5
- AngosLocalInstance runs Angos (M1) (#1) - ([24aeceb](https://github.com/batleforc/weebo-angos/commit/24aeceb3db89ec6e2a6015a2e1041a6f895d4494)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- local instances only, Angos reason codes, chart rules (M0) - ([b2ac1e9](https://github.com/batleforc/weebo-angos/commit/b2ac1e97916fb4aafe88f2bca46456d5d48a6afe)) - Max Batleforc, Claude Opus 5.5
#### Documentation
- Fumadocs site on @weebo/operator-docs (install guide, generated CRD and operator reference) - ([a5dd6d1](https://github.com/batleforc/weebo-angos/commit/a5dd6d1639961db191375389fccd3eb20bfbffbd)) - Max Batleforc, Claude Opus 5.5
#### Continuous Integration
- kind e2e (smoke, CRD name rule, leader failover) gating publish - ([5847eee](https://github.com/batleforc/weebo-angos/commit/5847eee1cf13a3d9e9eb5cd72c6b7fdb133ea66d)) - Max Batleforc, Claude Opus 5.5
#### Miscellaneous Chores
- (**version**) v0.1.0-alpha.1 - ([32e2810](https://github.com/batleforc/weebo-angos/commit/32e28100d2d4a800cfe6ae6cade8c6f8c20bae49)) - Max Batleforc
- kit v0.17.0, Apache-2.0 license, cog.toml and mise.lock - ([e72007e](https://github.com/batleforc/weebo-angos/commit/e72007e3a46f21b327537fbb922153ca8cfa9fbb)) - Max Batleforc, Claude Opus 5.5
- generated from weebo-operator-kit templates/operator (v0.16.0, product Angos) - ([39871bd](https://github.com/batleforc/weebo-angos/commit/39871bdef90275e4fbd30da07f14cbc1b4864b7b)) - Max Batleforc, Claude Opus 5.5

- - -

## [v0.1.0-alpha.2](https://github.com/batleforc/weebo-angos/compare/v0.1.0-alpha.1..v0.1.0-alpha.2) - 2026-10-16
#### Bug Fixes
- an identity's hash and version read at once (#5) - ([a34f119](https://github.com/batleforc/weebo-angos/commit/a34f119106c4593b9418af4688b98a812c93fbec)) - Maxime, Claude Opus 5.5

- - -

## [v0.1.0-alpha.1](https://github.com/batleforc/weebo-angos/compare/39871bdef90275e4fbd30da07f14cbc1b4864b7b..v0.1.0-alpha.1) - 2026-10-16
#### Features
- (**domain**) Angos 1.12.3 configuration model, layers and validation (M0) - ([4ea4485](https://github.com/batleforc/weebo-angos/commit/4ea4485fdb5d2c4ecde49af470e9e0a7ca109c19)) - Max Batleforc, Claude Opus 5.5
- AngosIdentity, AngosMaintenance and the prune lock (M3) (#4) - ([a826043](https://github.com/batleforc/weebo-angos/commit/a826043418917384e1c9656b88be940803dbb381)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- AngosRepository, access vocabulary, CEL on Angos's engine (M2) (#3) - ([d8deeb1](https://github.com/batleforc/weebo-angos/commit/d8deeb18108a2130e7571297e8d044414da6b0e9)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- instance admission, disruptive-update gate, live S3/TLS/kube-auth e2e (M1 leftovers) (#2) - ([abc48a0](https://github.com/batleforc/weebo-angos/commit/abc48a04a2754fb37ffa64efd795dca19620c350)) - Maxime, Claude Opus 5.5
- AngosLocalInstance runs Angos (M1) (#1) - ([24aeceb](https://github.com/batleforc/weebo-angos/commit/24aeceb3db89ec6e2a6015a2e1041a6f895d4494)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- local instances only, Angos reason codes, chart rules (M0) - ([b2ac1e9](https://github.com/batleforc/weebo-angos/commit/b2ac1e97916fb4aafe88f2bca46456d5d48a6afe)) - Max Batleforc, Claude Opus 5.5
#### Documentation
- Fumadocs site on @weebo/operator-docs (install guide, generated CRD and operator reference) - ([a5dd6d1](https://github.com/batleforc/weebo-angos/commit/a5dd6d1639961db191375389fccd3eb20bfbffbd)) - Max Batleforc, Claude Opus 5.5
#### Continuous Integration
- kind e2e (smoke, CRD name rule, leader failover) gating publish - ([5847eee](https://github.com/batleforc/weebo-angos/commit/5847eee1cf13a3d9e9eb5cd72c6b7fdb133ea66d)) - Max Batleforc, Claude Opus 5.5
#### Miscellaneous Chores
- (**version**) v0.1.0-alpha.1 - ([32e2810](https://github.com/batleforc/weebo-angos/commit/32e28100d2d4a800cfe6ae6cade8c6f8c20bae49)) - Max Batleforc
- kit v0.17.0, Apache-2.0 license, cog.toml and mise.lock - ([e72007e](https://github.com/batleforc/weebo-angos/commit/e72007e3a46f21b327537fbb922153ca8cfa9fbb)) - Max Batleforc, Claude Opus 5.5
- generated from weebo-operator-kit templates/operator (v0.16.0, product Angos) - ([39871bd](https://github.com/batleforc/weebo-angos/commit/39871bdef90275e4fbd30da07f14cbc1b4864b7b)) - Max Batleforc, Claude Opus 5.5

- - -

## [v0.1.0-alpha.1](https://github.com/batleforc/weebo-angos/compare/39871bdef90275e4fbd30da07f14cbc1b4864b7b..v0.1.0-alpha.1) - 2026-10-16
#### Features
- (**domain**) Angos 1.12.3 configuration model, layers and validation (M0) - ([4ea4485](https://github.com/batleforc/weebo-angos/commit/4ea4485fdb5d2c4ecde49af470e9e0a7ca109c19)) - Max Batleforc, Claude Opus 5.5
- AngosIdentity, AngosMaintenance and the prune lock (M3) (#4) - ([a826043](https://github.com/batleforc/weebo-angos/commit/a826043418917384e1c9656b88be940803dbb381)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- AngosRepository, access vocabulary, CEL on Angos's engine (M2) (#3) - ([d8deeb1](https://github.com/batleforc/weebo-angos/commit/d8deeb18108a2130e7571297e8d044414da6b0e9)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- instance admission, disruptive-update gate, live S3/TLS/kube-auth e2e (M1 leftovers) (#2) - ([abc48a0](https://github.com/batleforc/weebo-angos/commit/abc48a04a2754fb37ffa64efd795dca19620c350)) - Maxime, Claude Opus 5.5
- AngosLocalInstance runs Angos (M1) (#1) - ([24aeceb](https://github.com/batleforc/weebo-angos/commit/24aeceb3db89ec6e2a6015a2e1041a6f895d4494)) - Maxime, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5, Claude Opus 5.5
- local instances only, Angos reason codes, chart rules (M0) - ([b2ac1e9](https://github.com/batleforc/weebo-angos/commit/b2ac1e97916fb4aafe88f2bca46456d5d48a6afe)) - Max Batleforc, Claude Opus 5.5
#### Documentation
- Fumadocs site on @weebo/operator-docs (install guide, generated CRD and operator reference) - ([a5dd6d1](https://github.com/batleforc/weebo-angos/commit/a5dd6d1639961db191375389fccd3eb20bfbffbd)) - Max Batleforc, Claude Opus 5.5
#### Continuous Integration
- kind e2e (smoke, CRD name rule, leader failover) gating publish - ([5847eee](https://github.com/batleforc/weebo-angos/commit/5847eee1cf13a3d9e9eb5cd72c6b7fdb133ea66d)) - Max Batleforc, Claude Opus 5.5
#### Miscellaneous Chores
- kit v0.17.0, Apache-2.0 license, cog.toml and mise.lock - ([e72007e](https://github.com/batleforc/weebo-angos/commit/e72007e3a46f21b327537fbb922153ca8cfa9fbb)) - Max Batleforc, Claude Opus 5.5
- generated from weebo-operator-kit templates/operator (v0.16.0, product Angos) - ([39871bd](https://github.com/batleforc/weebo-angos/commit/39871bdef90275e4fbd30da07f14cbc1b4864b7b)) - Max Batleforc, Claude Opus 5.5

- - -

Changelog generated by [cocogitto](https://github.com/cocogitto/cocogitto).