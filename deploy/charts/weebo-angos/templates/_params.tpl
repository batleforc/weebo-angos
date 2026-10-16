{{/*
This operator's parameters for the kit's library chart (weebo-operator-lib,
see its README): everything templates/operator.yaml renders that is not a
value.
*/}}
{{- define "operator.params" -}}
group: angos.weebo.io
versions: [v1alpha1]
# Every kind the operator watches (ClusterRole).
kinds:
  - angosinstances
  - angoslocalinstances
  - angosobjects
# The namespaced kinds: what a team may manage (rbac.namespacedExample),
# and the watch Role's (watch.namespace).
namespacedKinds:
  - angoslocalinstances
  - angosobjects
# Kinds the admission webhook checks: none until the operator serves
# `/validate` (a namespace allow-list).
admitted: []
# The operator's own environment variables.
env: []
egress:
  - comment: Angos instances.
    rules: {{ toJson .Values.networkPolicy.egress.angos }}
# Alerts on the operator's own metrics, appended to the kit's.
alerts: []
{{- end -}}
