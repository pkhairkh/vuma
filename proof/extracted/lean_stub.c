/*
 * lean_stub.c — STUB linkage target for the Lean FFI surface.
 *
 * This file is compiled by build.rs (Wave 4-D) into the static archive
 * `liblean_extraction.a` whenever the real Lean → C extraction pipeline
 * (`lake build` → `lean --emit-c`) is unavailable or fails. It defines
 * the 7 Lean `@[export]` symbols documented in FFI_BRIDGE_PLAN.md §1 so
 * the Rust binary links cleanly in Lean-free environments.
 *
 * IMPORTANT — this is a LINKAGE STUB, not a faithful translation:
 *   * The return values are HARDCODED placeholders. They are never read
 *     in production because build.rs does NOT emit `lean_ffi_linked` on
 *     the stub path, so verification.rs keeps using the hand-written
 *     Rust verifiers (the parity-tested path). The stub only exists to
 *     satisfy the linker for `extern "C"` declarations in
 *     src/codegen/src/runtime/pmt_check.rs::lean_ffi.
 *   * Argument types use `void*` (matching `lean_object*` / Rust's
 *     `*mut LeanObject = *mut c_void`) so the stub does not depend on
 *     the Lean runtime headers. Real extraction emits `lean_object*`
 *     and requires linking `lean_runtime` (see FFI_BRIDGE_PLAN §2).
 *
 * The 7 symbols (matching `@[export ...]` in proof/PMT/Extraction.lean):
 *   1. lean_verified_capacity_check      — capacity-style check   → 0
 *   2. lean_verified_field_bounds_check  — capacity-style check   → 0
 *   3. lean_verified_linearity_check     — capacity-style check   → 0
 *   4. lean_verified_pmt_check           — composed capacity chk  → 0
 *   5. lean_verify_transform             — state verifier         → 1 (true)
 *   6. lean_verify_state_reads           — state verifier         → 1 (true)
 *   7. lean_verify_state_writes          — state verifier         → 1 (true)
 *
 * The capacity-style checks return 0 (fail-closed) and the state
 * verifiers return 1 (true) per the Wave 4-D stub contract. These
 * values are inert under the stub path (see note above).
 */

#include <stdint.h>

/* Opaque Lean object pointer — matches `lean_object*` / Rust `*mut c_void`. */
typedef void lean_object;

/* ── Group A: PMT capacity/bounds/linearity checks (return 0) ─────── */

uint8_t lean_verified_capacity_check(lean_object *used,
                                     lean_object *size,
                                     lean_object *capacity) {
    (void)used; (void)size; (void)capacity;
    return 0;
}

uint8_t lean_verified_field_bounds_check(lean_object *f,
                                         lean_object *layout) {
    (void)f; (void)layout;
    return 0;
}

uint8_t lean_verified_linearity_check(lean_object *var,
                                      lean_object *consumed) {
    (void)var; (void)consumed;
    return 0;
}

uint8_t lean_verified_pmt_check(lean_object *used,
                                lean_object *capacity,
                                lean_object *f,
                                lean_object *layout,
                                lean_object *var,
                                lean_object *consumed) {
    (void)used; (void)capacity; (void)f; (void)layout; (void)var; (void)consumed;
    return 0;
}

/* ── Group B: IVE state verifiers (return 1 = true) ───────────────── */
/*
 * NOTE: the Rust extern block in pmt_check.rs::lean_ffi declares
 * `lean_verify_transform(t: *mut LeanObject)` with ONE argument, while
 * the Lean `@[export lean_verify_transform]` takes TWO (layouts, t).
 * This stub matches the RUST extern arity (the actual link contract);
 * resolving the Lean/Rust arity mismatch is Wave 5 work (FFI_BRIDGE_PLAN
 * §3). The stub is never called at runtime, so the arity is inert here.
 */

uint8_t lean_verify_transform(lean_object *t) {
    (void)t;
    return 1;
}

uint8_t lean_verify_state_reads(lean_object *env_list,
                                lean_object *reads) {
    (void)env_list; (void)reads;
    return 1;
}

uint8_t lean_verify_state_writes(lean_object *env_list,
                                 lean_object *consumed,
                                 lean_object *writes) {
    (void)env_list; (void)consumed; (void)writes;
    return 1;
}

/* ─────────────────────────────────────────────────────────────────────
 * Wave 5-C — `_prim` primitive-signature wrappers (Extraction.lean §9).
 *
 * Wave 4-A added 7 `@[export ..._prim]` wrappers with C-marshallable
 * signatures (UInt64 unboxed; String stays boxed `lean_object*`). These
 * stub definitions mirror the originals so behavioral FFI smoke tests
 * (`tests/pmt_runtime_ffi_smoke.rs`) link and exercise the Rust→C ABI
 * plumbing end-to-end against C-compiled (NOT Lean-computed) returns.
 *
 * Returns mirror the non-`_prim` siblings above: the four
 * `verified_*_prim` capacity-style checks return 0 (fail-closed); the
 * three `verify_*_prim` state verifiers return 1 (true). C ABI follows
 * the Lean `@[export]` docstrings (UInt64 → uint64_t, String → boxed
 * lean_object*, Bool → uint8_t).
 * ───────────────────────────────────────────────────────────────────── */

uint8_t lean_verified_capacity_check_prim(uint64_t used,
                                          uint64_t size,
                                          uint64_t capacity) {
    (void)used; (void)size; (void)capacity;
    return 0;
}

uint8_t lean_verified_field_bounds_check_prim(uint64_t offset,
                                              uint64_t size,
                                              uint64_t total_size) {
    (void)offset; (void)size; (void)total_size;
    return 0;
}

uint8_t lean_verified_linearity_check_prim(lean_object *var,
                                           lean_object *consumed) {
    (void)var; (void)consumed;
    return 0;
}

uint8_t lean_verified_pmt_check_prim(uint64_t used,
                                     uint64_t capacity,
                                     uint64_t offset,
                                     uint64_t size,
                                     uint64_t total_size,
                                     lean_object *var,
                                     lean_object *consumed) {
    (void)used; (void)capacity; (void)offset; (void)size;
    (void)total_size; (void)var; (void)consumed;
    return 0;
}

uint8_t lean_verify_transform_prim(lean_object *registry,
                                   lean_object *input_layout,
                                   lean_object *output_layout) {
    (void)registry; (void)input_layout; (void)output_layout;
    return 1;
}

uint8_t lean_verify_state_reads_prim(lean_object *registry,
                                     lean_object *reads) {
    (void)registry; (void)reads;
    return 1;
}

uint8_t lean_verify_state_writes_prim(lean_object *registry,
                                      lean_object *consumed,
                                      lean_object *writes) {
    (void)registry; (void)consumed; (void)writes;
    return 1;
}
