# Multi-byte Stores and Computed Addresses

Tests whose primary characteristic is writing multi-byte values (u32, u64) as sequences of byte stores, or writing to addresses computed from complex expressions. These exercise the codegen's store-lowering paths and the IVE bounds analysis on non-trivial address expressions.

## What belongs here

- u32 written as 4 little-endian bytes (`cs_u32_le_store`)
- u32 reconstructed from 4 loaded bytes (`cs_u32_le_load`)
- Byte-level extraction from a wider value (`cs_byte_extraction`)
- Multi-byte scatter / gather across offsets (`cs_scatter_store`, `cs_struct_store`)
- Memset-like pattern fills (`cs_pattern_fill`, `cs_zero_fill`)
- Single-byte copies, swaps, and chains across buffers
- Computed destination addresses (`*(buf + N)` with constant N)

All tests are **straight-line code only** — no `if`/`else`, no `for`/`while`.
This avoids two known VUMA bugs (if-body and loop-body assignment
propagation) and isolates the store-lowering code path.

## Implementation notes / workarounds

- **Stored values are kept ≤ 255** to avoid the known U8-truncation bug
  that caps every `Store`/`Load` at U8 width on the current x86_64
  backend.
- **Shifts are loaded into variables** (e.g. `shift8: u32 = 8;`) to
  avoid the known "bitwise op with immediate RHS dropped" bug.
- **Pointer stores of bare scalar variables are miscompiled** on the
  current x86_64 backend (`*(p) = v;` is silently dropped). Four tests
  work around this by either storing an expression directly
  (`*(p) = a + b;`) or by using the identity-expression trick
  (`*(p) = v | 0;`). These workarounds are documented in the affected
  test files (cs_copy_bytes, cs_swap_bytes, cs_store_chain,
  cs_compute_then_store).

## Files (22)

### Existing (carried over)

- [`base64_encode.vuma`](base64_encode.vuma) — RFC 4648 Base64 encoder
- [`test_sha_manual.vuma`](test_sha_manual.vuma) — manual SHA-256 round

### New `cs_*` tests (20) — straight-line byte/u32 store patterns

- [`cs_byte_store.vuma`](cs_byte_store.vuma) — 42
- [`cs_u32_le_store.vuma`](cs_u32_le_store.vuma) — 120 (0x78)
- [`cs_u32_le_load.vuma`](cs_u32_le_load.vuma) — 42
- [`cs_multi_store.vuma`](cs_multi_store.vuma) — 10
- [`cs_overwrite_store.vuma`](cs_overwrite_store.vuma) — 99
- [`cs_scatter_store.vuma`](cs_scatter_store.vuma) — 6
- [`cs_struct_store.vuma`](cs_struct_store.vuma) — 3
- [`cs_pattern_fill.vuma`](cs_pattern_fill.vuma) — 7
- [`cs_zero_fill.vuma`](cs_zero_fill.vuma) — 0
- [`cs_copy_bytes.vuma`](cs_copy_bytes.vuma) — 42
- [`cs_swap_bytes.vuma`](cs_swap_bytes.vuma) — 1
- [`cs_shift_left.vuma`](cs_shift_left.vuma) — 0
- [`cs_independent_stores.vuma`](cs_independent_stores.vuma) — 42
- [`cs_store_chain.vuma`](cs_store_chain.vuma) — 55
- [`cs_store_after_alloc.vuma`](cs_store_after_alloc.vuma) — 42
- [`cs_store_before_free.vuma`](cs_store_before_free.vuma) — 42
- [`cs_multi_buf_stores.vuma`](cs_multi_buf_stores.vuma) — 3
- [`cs_compute_then_store.vuma`](cs_compute_then_store.vuma) — 10
- [`cs_store_load_store.vuma`](cs_store_load_store.vuma) — 99
- [`cs_byte_extraction.vuma`](cs_byte_extraction.vuma) — 66 (0x42)

## Verification

All 20 `cs_*` tests **PASS** (exit code matches the documented expected
value) on the current x86_64 backend, and are deterministic across
multiple runs (3× verified).
