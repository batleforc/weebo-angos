//! An `AngosLocalInstance`: its workload and configuration. Until the
//! workload lands (milestone M1), reconciling it only checks its spec.

use api::LocalInstance;

use crate::use_cases::Outcome;

pub fn reconcile_instance(_instance: &LocalInstance) -> Outcome {
    Outcome::synced("the instance's spec is valid; no workload is run yet")
}

#[cfg(test)]
mod tests {
    use api::local_instance::LocalInstanceSpec;
    use domain::error::ReasonCode;

    use super::*;

    #[test]
    fn an_instance_is_ready() {
        let instance = LocalInstance::new("reg", LocalInstanceSpec::default());
        assert_eq!(
            reconcile_instance(&instance).reason(),
            ReasonCode::Reconciled
        );
    }
}
