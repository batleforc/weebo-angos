//! Pure domain of the operator: no Kubernetes, no HTTP — the kit's
//! building blocks, Angos's configuration model (`config`) and the rules
//! Angos checks it with (`validation`). Whatever the operator decides
//! without I/O lives here.

/// `Debug` for a struct holding secrets: every field listed, the secret
/// ones printed `<redacted>`. The destructuring is exhaustive: a field
/// added to the struct must be listed here too.
#[macro_export]
macro_rules! redacted_debug {
    ($ty:ident { $($field:ident),* $(,)? } secrets { $($secret:ident),* $(,)? }) => {
        impl ::std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let Self { $($field,)* $($secret: _,)* } = self;
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($field), $field))*
                    $(.field(stringify!($secret), &"<redacted>"))*
                    .finish()
            }
        }
    };
}

pub mod access;
pub mod config;
pub mod error;
pub mod validation;

/// A `Ready: False` cause, carrying one of this operator's reason codes.
pub type Failure = weebo_kit_domain::Failure<error::ReasonCode>;
/// A non-blocking condition shown next to `Ready: True`.
pub type Advisory = weebo_kit_domain::Advisory<error::ReasonCode>;
