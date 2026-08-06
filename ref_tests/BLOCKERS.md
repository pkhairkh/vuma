# vuma v0.2.0-alpha.15 Codegen Blockers

Discovered during Wave 1 (MD5 rewrite + testing) on 2026-08-07.
These are vuma COMPILER bugs, not womb module bugs. They block the
womb/* spec-compliance refactor effort.

## Summary

ALL 44 womb/crypto/* modules use old pointer syntax (`allocate`, `free`,
`Address`, `*ptr`) that VUMA 2.0's PMT-only parser rejects. The modules
must be rewritten in PMT syntax (`state_new(Layout)`, `State<T>`,
`[u8; N]` byte indexing).

The PMT rewrite itself is straightforward (proven by womb/kernel/crypto/sha.vuma
W48 which is PMT-native). However, the v0.2.0-alpha.15 codegen has
**multiple interacting bugs** that prevent non-trivial PMT crypto code
from compiling and running correctly.

## Bug Catalog

### BUG-1: Array index arithmetic on parameters fails silently

```vuma
// FAILS — base + 1 evaluates to base (addition ignored)
transform write_word(buf: State<Buf>, base: u32, val: u32) {
    buf.data[base]     = (val & 255) as u8;
    buf.data[base + 1] = ((val >> 8) & 255) as u8;  // writes to [base], not [base+1]
}
```

**Workaround**: Use literal indices only. Precompute loop-variable-derived
indices into local vars before array access:
```vuma
let off = j * 4;   // local var — works
buf.data[off]      // OK
```

### BUG-2: state_new() inside transforms called from other transforms fails

```vuma
// FAILS — "ir: unknown variable 'data' referenced in SCG"
transform md5_oneshot(data: State<Md5Data>, len: u32, out: State<Md5Digest>) {
    let ctx = state_new(Md5Ctx);    // allocating inside a called transform
    md5_init(ctx);
    md5_update(ctx, data, len);     // passing State through 2 levels fails
    md5_final(ctx, out);
}
```

**Workaround**: Allocate all State<T> in `main()` and call init/update/final
directly from main. Do not wrap them in a higher-level function.

### BUG-3: Import resolution fails for complex transforms

```vuma
// Compiles OK, but binary SIGILLs at ud2 (FFI fallback stub)
import "womb/kernel/crypto/sha.vuma"::{sha256_init, sha256_update, sha256_final};
```

The x86_64 codegen resolves calls to imported complex transforms to the
`ud2` (undefined instruction) FFI fallback stub at address 0x41075b.
Simple imported transforms (e.g. `trivial_get_42`) work; complex ones
(with loops, state_new, multiple State params) do not.

**Workaround**: Use self-contained .vuma files (no imports). Concatenate
module + test driver into a single file.

### BUG-4: "unsupported FieldAccess" warnings null out State fields

Compiling transforms that take `State<T>` parameters AND access u32/u64
fields (not just `[u8; N]` arrays) triggers:
```
[vuma] WARNING: unsupported FieldAccess (not state-typed) in flatten_expr; using 0
[vuma] WARNING: state_new() outside let-binding in flatten_expr; using 0
```

The warned-about expressions are silently replaced with 0 (null), causing
SIGSEGV at runtime. This affects `md5_update`, `md5_final`, `md5_oneshot`,
and `md5_add_bits` even when they are never called from main.

**Workaround**: Strip these transforms from the module before compiling.
Inline their logic in the test driver's `main()`.

### BUG-5: Presence/absence of a `while` loop affects subsequent code correctness

```vuma
// WORKS (has while loop before field reads):
transform main() -> i32 {
    let ctx = state_new(Md5Ctx);
    md5_init(ctx);
    ctx.buf[0] = 128;
    let i: u32 = 1;
    while i < 56 { ctx.buf[i] = 0; i = i + 1; }
    md5_compress(ctx);
    if ctx.state[0] != 212 { return 1; }  // OK — reads correctly
    return 0;
}

// CRASHES (no while loop before field reads):
transform main() -> i32 {
    let ctx = state_new(Md5Ctx);
    md5_init(ctx);
    ctx.buf[0] = 128;
    md5_compress(ctx);
    if ctx.state[0] != 212 { return 1; }  // SIGSEGV
    return 0;
}
```

Likely a register allocation or code alignment issue in the x86_64 backend.

**Workaround**: Always include a `while` loop in main() before reading
State fields after md5_compress.

### BUG-6: Code formatting sensitivity

The same MD5 implementation in two formatting styles produces different
results: compact (one-line layouts, packed consts) works; verbose
(multi-line layouts, one const per line, comments inside transforms)
crashes. Root cause not yet determined — possibly parser position tracking
or SCG node ID allocation differences.

**Workaround**: Use compact formatting for all womb modules pending
compiler fix.

## Impact on Wave Plan

- **Wave 1 (MD5 + SHA-1)**: MD5 rewritten in PMT syntax, verified correct
  for MD5("") and MD5("abc") via manual test drivers. Batch 100-vector
  testing blocked by BUG-4/BUG-5 (harness-generated drivers crash
  unpredictably). SHA-1 not yet started.
- **Waves 2-18**: Blocked until BUG-1 through BUG-6 are fixed in the
  vuma compiler.

## Recommended Path Forward

1. **Fix BUG-1** (array index arithmetic on parameters) in
   `src/codegen/src/x86_64/reg_isel.rs` — likely a missing `Add` emission
   for parameter-derived indices.
2. **Fix BUG-2** (State pass-through) in `src/codegen/src/scg_to_ir.rs` —
   the SCG→IR bridge loses State-typed parameter bindings across call
   boundaries.
3. **Fix BUG-3** (import resolution) in `src/parser/src/resolver.rs` —
   imported transforms' call targets are not patched correctly.
4. **Fix BUG-4** (FieldAccess on non-array fields) in `src/pipeline.rs`
   `flatten_expr` — u32/u64 field reads on State<T> are not recognized.
5. **Fix BUG-5** (loop-dependent correctness) — investigate register
   allocation / liveness analysis in the x86_64 backend.
6. **Fix BUG-6** (formatting sensitivity) — investigate parser position
   tracking.

Until these are fixed, the womb module rewrite can proceed incrementally
(module by module, with manual test drivers), but the 100-vector-per-module
DoD cannot be met via automated batch testing.
