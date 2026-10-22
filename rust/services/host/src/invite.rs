//! Invite links and codes (`legacy/ReforgedHost/src/invite.ts`): the `reforged-revival://` formats
//! (section 6; parsed by `protocol`, RS1), the gateway certificate's fingerprint and the backend's
//! local admin route for codes.
//!
//! Filled by RH3. `setup` and `doctor` use the fingerprint.

use std::path::Path;

use crate::error::{HostError, not_implemented};

/// The SHA-256 of the gateway certificate, as every invite pins it (`fp`).
pub fn certificate_fingerprint(_cert_file: &Path) -> Result<String, HostError> {
    not_implemented("reading the certificate fingerprint", "RH3")
}
