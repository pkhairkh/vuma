/* vuma_vm.h — the VUMA C-API for foreign callbacks.
 *
 * Generalizes the wasm32 host shim (scripts/wasm32_runner.py:make_host_functions)
 * into a C header shipped for all backends. When a C library calls back into
 * VUMA (e.g. sqlite3_exec's row callback), it receives a vuma_context_t* and
 * uses these accessors to safely interact with VUMA's ___pmt_buffer.
 *
 * RE-ENTRANCY RULE (decided):
 *   Callbacks run on an isolated callback stack with their own scratchpad
 *   frame. They are FORBIDDEN from touching any State in the caller's live
 *   set (enforced by callback_live_set — trap on violation). They may only:
 *     - state_new their own fresh states (at new offsets in ___pmt_buffer)
 *     - read/write those own states
 *     - return scalars via vuma_push_*
 *
 * SACRED INVARIANT:
 *   The callback's state_new allocations go into ___pmt_buffer at fresh
 *   offsets (the buffer is sized for the union of all states, including
 *   callback-reachable ones, at compile time). The caller's in-flight
 *   ___pmt_buffer region is NOT touched.
 */

#ifndef VUMA_VM_H
#define VUMA_VM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle to the VUMA callback context. */
typedef struct vuma_context vuma_context_t;

/* ── Read/write VUMA's ___pmt_buffer at a typed offset ────────────────────
 *
 * These are the ONLY safe way for C callbacks to touch VUMA memory.
 * The offset is relative to ___pmt_buffer_base. The callback_live_set
 * guard traps if the offset falls within a caller-live region.
 */

uint32_t vuma_read_u32 (vuma_context_t *ctx, uint64_t offset);
uint64_t vuma_read_u64 (vuma_context_t *ctx, uint64_t offset);
void     vuma_write_u32(vuma_context_t *ctx, uint64_t offset, uint32_t val);
void     vuma_write_u64(vuma_context_t *ctx, uint64_t offset, uint64_t val);

/* ── Allocate a fresh state in ___pmt_buffer ──────────────────────────────
 *
 * Returns the offset of the new state. The callback owns this state until
 * it returns. The allocation is at a fresh offset (not aliased with any
 * caller-live region).
 */

uint64_t vuma_state_new(vuma_context_t *ctx, const char *layout_name);

/* ── Push a scalar return value back to VUMA ──────────────────────────────
 *
 * The callback's return value is pushed via these functions. The last
 * pushed value before the callback returns is the callback's result.
 */

void vuma_push_i32(vuma_context_t *ctx, int32_t val);
void vuma_push_i64(vuma_context_t *ctx, int64_t val);

#ifdef __cplusplus
}
#endif

#endif /* VUMA_VM_H */
