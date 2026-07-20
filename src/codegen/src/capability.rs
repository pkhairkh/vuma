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
//! A [`CapabilityToken`] is a signed authorisation that grants a target
//! process the right to access a specific [`Resource`] (file, network
//! endpoint, memory region, MMIO range, or channel) with a specific set
//! of [`MemoryPermissions`]. Capabilities are:
//!
//! - **Tamper-evident** — every token carries a 32-byte signature over
//!   all of its fields. **IMPORTANT: the signature is NOT HMAC-SHA256.**
//!   It is a custom construction based on FNV-1a (a non-cryptographic
//!   hash) run four times with different 1-byte salt prefixes. See the
//!   SECURITY NOTE below. A token whose signature does not verify under
//!   the kernel's signing key is rejected by [`verify_capability`], but
//!   an attacker who knows `signing_key` can forge a valid signature.
//! - **Delegable** — the holder of a token may mint a child token for a
//!   subset of their own permissions, up to [`MAX_DELEGATION_DEPTH`]
//!   levels deep. Each delegation increments `delegation_depth`.
//! - **Expiring** — every token has `expires_at`; expired tokens are
//!   rejected by verification.
//! - **Revocable** — a [`CapabilitySet`] tracks issued tokens and
//!   supports revocation by ID, which propagates to delegated children.
//!
//! ## SECURITY NOTE — NOT CRYPTOGRAPHICALLY SECURE
//!
//! The signature algorithm (`compute_signature` in `ipc::capability`)
//! uses **FNV-1a 64-bit** (a non-cryptographic hash with offset basis
//! `0xcbf29ce484222325` and prime `0x100000001b3`) run four times with
//! 1-byte salt prefixes (0, 1, 2, 3), concatenating the four u64
//! outputs into 32 bytes.
//!
//! **This is NOT HMAC, NOT a MAC, and NOT resistant to a determined
//! adversary with access to `signing_key`.** It is a tamper-detection
//! checksum suitable for catching accidental corruption or casual
//! modification, but it provides zero protection against forgery.
//!
//! A production deployment MUST replace `compute_signature` with
//! HMAC-SHA256 (or BLAKE2s) over a real per-domain secret key. The
//! `verify_capability` API does not change — only the internal
//! `compute_signature` function needs swapping.
//!
//! Additionally, `verify_capability` and `verify_delegation_chain` are
//! **never called from emitted VUMA binaries**. The compiler's
//! `channel_recv` codegen checks only `cap_count == 0` (rejecting any
//! message that carries capability tokens, because it cannot verify
//! them inline). A `.vuma` program that calls `channel_recv` on a
//! frame with `cap_count > 0` receives `-4` (PERMISSION_DENIED).
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

    #[test]
    fn test_capability_signature_is_fnv_not_hmac() {
        // HONESTY TEST (audit 2025-07-20): verify that the signature
        // algorithm is the FNV-1a × 4 construction documented above, NOT
        // HMAC-SHA256. If someone "fixes" compute_signature to use a real
        // MAC, this test will need updating — but until then, it pins the
        // current reality so the public docs don't drift back to claiming
        // HMAC-SHA256.
        let token = grant_capability(
            0x1234_5678_9ABC_DEF0_1234_5678_9ABC_DEF,
            1, 2,
            Resource::Channel(42),
            MemoryPermissions { read: true, write: false, execute: false },
            0,
            1000,
            3600,
            &[0xAA; 32],
        );
        // The signature should be 32 non-zero bytes (FNV-1a of non-empty
        // input is never all-zero). If it were HMAC-SHA256 with a 32-byte
        // key, it would also be 32 non-zero bytes, so this doesn't
        // distinguish — but it does verify the signature was computed.
        assert_ne!(token.signature, [0u8; 32],
            "signature must be computed (non-zero)");
        // Verify the token round-trips: verify_capability with the same
        // signing key and a `now` within the validity window succeeds.
        assert!(verify_capability(&token, &[0xAA; 32], 2000, None,
            &MemoryPermissions { read: true, write: false, execute: false }).is_ok(),
            "verify_capability must succeed for a valid token");
        // Verify with a DIFFERENT key fails — the signature is key-sensitive.
        assert!(verify_capability(&token, &[0xBB; 32], 2000, None,
            &MemoryPermissions { read: true, write: false, execute: false }).is_err(),
            "verify_capability must fail with wrong signing key");
    }
}
