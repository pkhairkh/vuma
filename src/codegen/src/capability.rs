//! Capability tokens (Wave 11 — L2 Runtime Encapsulation).
//!
//! This module is the canonical home for capability-token types and
//! operations. The implementations live in [`crate::ipc::capability`]
//! (added in a prior wave); this file re-exports them so that the
//! compiler pipeline can `use crate::capability::*` without reaching
//! into the `ipc` module's internals.
//!
//! ## Overview
//!
//! A [`CapabilityToken`] is a signed, unforgeable authorisation that
//! grants a target process the right to access a specific [`Resource`]
//! (file, network endpoint, memory region, MMIO range, or channel) with
//! a specific set of [`MemoryPermissions`]. Capabilities are:
//!
//! - **Unforgeable** — every token carries an HMAC-SHA256 signature over
//!   all of its fields; a token whose signature does not verify under the
//!   kernel's signing key is rejected by [`verify_capability`].
//! - **Delegable** — the holder of a token may mint a child token for a
//!   subset of their own permissions, up to [`MAX_DELEGATION_DEPTH`]
//!   levels deep. Each delegation increments `delegation_depth`.
//! - **Expiring** — every token has `expires_at`; expired tokens are
//!   rejected by verification.
//! - **Revocable** — a [`CapabilitySet`] tracks issued tokens and
//!   supports revocation by ID, which propagates to delegated children.
//!
//! ## Wire format
//!
//! Each token serialises to a fixed [`CAPABILITY_TOKEN_SIZE`]-byte
//! little-endian record (see `ipc::capability::CapabilityToken::encode`
//! / `decode`). The L1 message framer slices `cap_count` such records
//! out of the capability section of an [`crate::ipc::EncapsulatedMessage`].

pub use crate::ipc::capability::{
    Resource, MemoryPermissions, CapabilityToken, CapabilitySet,
    CAPABILITY_TOKEN_SIZE, RESOURCE_OFFSET, RESOURCE_FIELD_SIZE,
    MAX_RESOURCE_STRING, MAX_DELEGATION_DEPTH,
};

// Re-export the grant/verify/delegate functions (Wave 11b/11c/11d) and
// the delegation-chain verifier (Wave 33+) for pipeline consumers.
pub use crate::ipc::capability::{
    grant_capability, verify_capability, verify_delegation_chain,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_module_re_exports_compile() {
        // Smoke test: the re-exported symbols are reachable and have the
        // expected types. This is the acceptance test for Wave 11a —
        // "Capability module exists" and "CapabilityToken struct defined".
        let _ = MemoryPermissions::default();
        let _ = MAX_DELEGATION_DEPTH;
        let _ = CAPABILITY_TOKEN_SIZE;
        // Resource variants exist:
        let _ = Resource::File("/tmp/test".to_string());
        let _ = Resource::Network("127.0.0.1".to_string(), 8080);
        let _ = Resource::Memory(0x1000, 0x2000);
        let _ = Resource::Mmio(0xFE00_0000, 0x1000);
        let _ = Resource::Channel(42);
    }
}
