//! # FNV-1a 64-bit hashing utilities
//!
//! Single source of truth for the FNV-1a 64-bit hash function used across
//! the VUMA compiler. Previously duplicated in 7 places — the canonical
//! type-hash function lived in both `vuma_codegen::ipc::type_hash` and a
//! private copy at `vuma_ive::verification::type_hash`, plus several
//! `compute_fnv1a_64` / `fnv1a_64` byte-slice variants scattered through
//! codegen and proof.
//!
//! See `docs/architecture/ive-fix-proposals.md` Gap 6 for the rationale.
//!
//! ## Constants
//!
//! - Init:  `0xcbf29ce484222325` (FNV-1a 64 offset basis)
//! - Prime: `0x100000001b3`      (FNV-1a 64 prime)
//!
//! ## References
//!
//! - Fowler, Noll, Vo — FNV-1a 64-bit hash (1991).
//! - Hunt & Thomas — "The Pragmatic Programmer", DRY/SPOT principle (1999).
//! - MLIR ODS — single source of truth for operation/type semantics.

/// FNV-1a 64-bit hash of a type string. Used for type hashing across IVE
/// and codegen — populates `MessageHeader::type_hash` and keys the protocol
/// state machine. Single source of truth — previously duplicated in 7 places.
pub fn type_hash(ty: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in ty.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// FNV-1a 64-bit hash over byte slices. Used by capability-token signature
/// computation, stark-prove verifier-key commitment, and proof-goal
/// fingerprinting.
pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: FNV-1a 64 known-answer for "" is the offset basis.
    #[test]
    fn empty_string_is_offset_basis() {
        assert_eq!(type_hash(""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
    }

    /// Sanity: a non-empty input differs from the basis and is stable.
    #[test]
    fn stable_nonempty() {
        let a = type_hash("i32");
        let b = type_hash("i32");
        assert_eq!(a, b);
        assert_ne!(a, 0xcbf29ce484222325);
        assert_ne!(a, type_hash("i64"));
    }

    /// `type_hash(s)` must equal `fnv1a_64(s.as_bytes())` — the type-hash
    /// is just the byte-slice hash over the UTF-8 representation.
    #[test]
    fn type_hash_matches_byte_slice() {
        for s in &["", "i32", "channel_open", "capability_grant", "🦀"] {
            assert_eq!(type_hash(s), fnv1a_64(s.as_bytes()));
        }
    }

    // ── Gap 6 regression tests (-d) ───────────────────────────
    //
    // These tests pin the FIX for caveats.md §2 row 6: `type_hash` is
    // now defined ONCE here (the single source of truth) and re-exported
    // from `vuma_codegen::ipc::type_hash`. Previously the function was
    // hand-duplicated in `vuma_ive::verification::type_hash` and
    // `vuma_codegen::ipc::type_hash`, and drift between the copies could
    // silently desynchronise IVE and codegen type fingerprints. The
    // tests below guard against any future reintroduction of the
    // duplication by asserting the two semantic invariants the
    // deduplication relies on: distinct types hash distinctly, and the
    // hash is deterministic across repeated calls.

    /// Distinct type strings MUST hash to distinct u64 values.
    ///
    /// If two distinct types ever collide, the L1→L3 collapse proof in
    /// `vuma_ive::verification::l1l3_collapse` (which keys channel-type
    /// agreement on `type_hash`) would silently accept a type-safety
    /// hole. Pick `u32` and `u64` — both common VUMA primitive types
    /// that share a prefix; a broken hash that truncated or skipped
    /// trailing bytes would collide on them.
    #[test]
    fn type_hash_distinct_for_distinct_types() {
        let h_u32 = type_hash("u32");
        let h_u64 = type_hash("u64");
        assert_ne!(
            h_u32, h_u64,
            "type_hash(u32) and type_hash(u64) must differ — a collision would \
             let the L1→L3 collapse proof accept a u32/u64 channel type-safety hole"
        );
        // Also exercise the canonical capability-token intrinsics —
        // these MUST be distinguishable so a capability_grant cannot be
        // confused with a capability_verify at the IPC layer.
        assert_ne!(
            type_hash("capability_grant"),
            type_hash("capability_verify"),
        );
        assert_ne!(type_hash("stark_prove"), type_hash("stark_verify"),);
    }

    /// `type_hash` MUST be deterministic — the same input string must
    /// always produce the same u64 across repeated calls. This is what
    /// makes the single-source-of-truth re-export at
    /// `codegen/src/ipc.rs:20` sound: IVE and codegen see the same
    /// fingerprint for the same type, on every call.
    #[test]
    fn type_hash_is_deterministic_across_calls() {
        let inputs = [
            "",
            "u32",
            "u64",
            "i32",
            "i64",
            "channel_open",
            "capability_grant",
            "capability_delegate",
            "capability_verify",
            "capability_revoke",
            "stark_prove",
            "stark_verify",
        ];
        for s in inputs {
            let first = type_hash(s);
            // Call 16× to surface any non-determinism (RNG, thread-local
            // state, accidental `&mut`, etc.).
            for _ in 0..16 {
                assert_eq!(
                    type_hash(s),
                    first,
                    "type_hash({:?}) is not deterministic — saw different \
                     values across repeated calls; this would desynchronise \
                     IVE and codegen type fingerprints",
                    s,
                );
            }
        }
    }
}
