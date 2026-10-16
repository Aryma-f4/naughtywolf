//! Shared wire type for harvested credentials.
//!
//! The implant serialises a `Vec<Harvested>` as the `nw/creds` task stdout;
//! the server parses it back into its credential store. Keeping the type in
//! `nw-profile` (which both sides already depend on) makes the contract
//! explicit rather than duplicated.

use serde::{Deserialize, Serialize};

/// One harvested credential.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Harvested {
    /// Where it came from, e.g. `ssh-key`, `cloud-aws`, `registry-putty`.
    pub source: String,
    /// Kind of secret, e.g. `private-key`, `api-token`, `password-hash`.
    pub cred_type: String,
    /// Identifying label, e.g. a path, `user@host`, or a service name.
    pub id: String,
    /// The secret material, or empty when only its presence matters.
    pub secret: String,
}

impl Harvested {
    pub fn new(
        source: &str,
        cred_type: &str,
        id: impl Into<String>,
        secret: impl Into<String>,
    ) -> Self {
        Self {
            source: source.to_owned(),
            cred_type: cred_type.to_owned(),
            id: id.into(),
            secret: secret.into(),
        }
    }
}
