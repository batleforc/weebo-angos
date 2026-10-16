//! Pure domain of the operator: no Kubernetes, no HTTP — the kit's
//! building blocks, Angos's configuration model (`config`) and the rules
//! Angos checks it with (`validation`). Whatever the operator decides
//! without I/O lives here.

pub mod config;
pub mod error;
pub mod validation;

/// A `Ready: False` cause, carrying one of this operator's reason codes.
pub type Failure = weebo_kit_domain::Failure<error::ReasonCode>;
/// A non-blocking condition shown next to `Ready: True`.
pub type Advisory = weebo_kit_domain::Advisory<error::ReasonCode>;
