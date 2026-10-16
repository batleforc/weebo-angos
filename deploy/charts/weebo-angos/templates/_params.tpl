{{/*
This operator's parameters for the kit's library chart (weebo-operator-lib,
see its README): everything templates/operator.yaml renders that is not a
value.
*/}}
{{- define "operator.params" -}}
{{- /* What the operator reads and writes in the namespaces of its instances
(operator.namespacedRules): cluster-wide in the ClusterRole, or — with
watch.namespace — only in that namespace, through the kit's watch Role
(params.watchRules), which a watch.clusterKinds: false release renders
too (spec §7). */ -}}
{{- $watchNamespace := include "weebo-operator.watchNamespace" . -}}
group: angos.weebo.io
versions: [v1alpha1]
# Every kind the operator watches (ClusterRole).
kinds:
  - angoslocalinstances
# The namespaced kinds — all of them, the operator has no cluster-scoped
# kind: what a team may manage (rbac.namespacedExample), and the watch
# Role's (watch.namespace).
namespacedKinds:
  - angoslocalinstances
# The operator reads and writes Secrets in every namespace that has an
# instance, also limited to one namespace: said explicitly.
clusterSecrets: true
# Kinds the admission webhook checks (`/validate`): a configuration Angos
# would refuse, pods the operator doesn't run, a Secret that doesn't opt in.
admitted:
  - angoslocalinstances
# Cluster-wide only: the namespaced rules below, and the labels of the
# namespaces repositories come from (repositorySelector.namespaces.from:
# Selector). Limited to one namespace, none of them: they go to
# watchRules. No Secret watch.
clusterRules:
  {{- if not $watchNamespace }}
  - apiGroups: [""]
    resources: [namespaces]
    verbs: [get, list, watch]
  {{- include "operator.namespacedRules" . | nindent 2 }}
  {{- end }}
# Limited to one namespace (watch.namespace): the same rules, in the watch
# Role of that namespace only.
watchRules:
  {{- if $watchNamespace }}
  {{- include "operator.namespacedRules" . | nindent 2 }}
  {{- end }}
# The operator's own environment variables.
env:
  - name: WEEBO_ANGOS_ALLOWED_IMAGES
    value: {{ join "," .Values.managedInstances.allowedImages | quote }}
  {{- with .Values.defaults.image }}
  - name: WEEBO_ANGOS_DEFAULT_IMAGE
    value: {{ . | quote }}
  {{- end }}
  - name: WEEBO_ANGOS_RELOAD_LOG_BYTES
    value: {{ .Values.defaults.reloadLogBytes | int64 | toString | quote }}
# The operator never calls Angos (no administration API): files, objects,
# pod logs and pod status only, all through the API server.
egress: []
# Alerts on the operator's own metrics, appended to the kit's.
alerts: []
{{- end -}}

{{/* The rules on the namespaced resources of the instances' namespaces:
what an instance is made of (server-side apply, each object read first to
never take over someone else's), the maintenance Jobs, the pods and their
logs (a reload's result is read there, spec D1), the ReplicaSets (which
startup Secret each still mounts), and the ConfigMaps a CA comes from. */}}
{{- define "operator.namespacedRules" -}}
- apiGroups: [apps]
  resources: [deployments]
  verbs: [get, list, watch, create, patch, delete]
- apiGroups: [apps]
  resources: [replicasets]
  verbs: [get, list, watch]
- apiGroups: [""]
  resources: [services, serviceaccounts, persistentvolumeclaims]
  verbs: [get, list, watch, create, patch, delete]
- apiGroups: [policy]
  resources: [poddisruptionbudgets]
  verbs: [get, list, watch, create, patch, delete]
- apiGroups: [networking.k8s.io]
  resources: [networkpolicies]
  verbs: [get, list, watch, create, patch, delete]
- apiGroups: [batch]
  resources: [jobs]
  verbs: [get, list, watch, create, patch, delete]
- apiGroups: [""]
  resources: [pods]
  verbs: [get, list, watch]
- apiGroups: [""]
  resources: [pods/log]
  verbs: [get]
- apiGroups: [""]
  resources: [configmaps]
  verbs: [get, list, watch]
{{- end -}}
