# TASKLIST — Domain 5: Compiler / Codegen Infrastructure

**Branch**: `agent-codegen-infra`
**Agent**: Domain-5 agent (compiler + codegen + multi-backend infrastructure)
**Scope**: All Rust source code under `src/` — NOT any `.vuma` module files

## File Scope (Agent MAY modify)

### Source files
- `src/bin/compile_dump.rs` — standalone compiler (749 lines)
- `src/codegen/src/*.rs` — codegen library (opt.rs = 5851 lines, ir.rs, regalloc.rs, etc.)
- `src/codegen/src/{x86_64,x86_32,aarch64,aarch64_be,arm32,armeb,riscv32,riscv64,mips64,mips64be,ppc64,ppc64le,loongarch64,s390x,sparc64,alpha,hppa,m68k,wasm32}/*.rs` — 19 backend instruction selectors
- `src/scg/src/*.rs` — SCG transform passes (transform.rs = 4324 lines)
- `src/parser/src/*.rs` — VUMA parser (parser.rs = 7825 lines, ast.rs, lexer.rs, resolver.rs, to_scg.rs)
- `src/pipeline.rs` — main compilation pipeline (10628 lines)
- `src/ive/src/*.rs` — invariant verification engine
- `src/vuma/src/*.rs` — core library
- `Cargo.toml`, `Cargo.lock` — Rust dependencies

## File Scope (Agent MAY NOT modify)

- `womb/crypto/**/*.vuma` — owned by Domains 1, 2, 3, 4
- `tests/compact_harnesses/*.vuma` — owned by module domain agents
- `test_results/standard_vectors/*.json` — owned by module domain agents
- `scripts/validate_compact.py` — read-only (used by all domains)

## Shared Files (Append-Only / Coordinate via PR)

- `test_results/compact_results.json` — codegen fixes affect ALL modules; coordinate with other domains via PR
- `worklog.md` — append your section

## Current State

Key findings from prior sessions:
1. **SCG Inliner `max_inline_size: 0`** (commit `efffd66e`): workaround for State<T> pass-by-ref bug. The `InliningPass::run` method in `src/scg/src/transform.rs` (line ~531) needs to map State<T> params to the caller's actual state offsets, not create fresh copies. Currently disabled — should be re-enabled (`max_inline_size: 50`) after the fix.
2. **E-graph skip implemented** (commit `08082a22`): `equality_saturation_with_cost` is now skipped for functions with >500 instructions. This cut ml_kem compile time from 72s to 33.5s.
3. **verify_between disabled** (commit `69631c64`): SCG validation between passes was disabled for 2x compile speedup. Should be re-enabled once SCG bugs are fixed.
4. **32-bit backends**: arm32, armeb, hppa, m68k, sparc64 have known issues with u64 operations (SHA-384/512/MD5 use u64).
5. **riscv32**: segfaults.
6. **wasm32**: `print_int` issue.
7. **x86_32**: 7 modules fail (u64 codegen).
8. **Register allocation** on huge `main` functions (from inlined code) takes 16s. Graph coloring is O(n²).

## Waves

### Wave 1: Fix SCG Inliner State<T> Pass-by-Reference Bug

**Approach**:
1. Read `src/scg/src/transform.rs` around line 531 (`InliningPass::run`)
2. Read the `InliningPass` struct and how it handles `State<T>` parameters
3. The bug: when a function with `State<T>` parameters is inlined, the State<T> parameter (an offset into the PMT buffer) is treated as a local copy rather than a reference
4. Fix: map State<T> params to the caller's actual state offsets during inlining
5. Re-enable `max_inline_size: 50` in `src/bin/compile_dump.rs` (line 174)
6. Re-enable `verify_between` in `src/scg/src/transform.rs` (line 1613: change `verify_between: false` to `true`)
7. Verify all 40 currently-PASSING modules still PASS 20/20 on x86_64 after the change
8. Verify ml_kem compiles correctly with inlining re-enabled (should be faster)

**DoD**:
- `max_inline_size: 50` in `src/bin/compile_dump.rs`
- `verify_between: true` in `src/scg/src/transform.rs`
- All 40 currently-PASSING modules still PASS 20/20 on x86_64
- ml_kem compile time ≤ 30s (was 33.5s with inlining disabled)

### Wave 2: Optimize Register Allocation for Large Functions

`allocate_registers` on huge `main` functions takes 16s (graph coloring is O(n²)).

**Approach**:
1. Read `src/codegen/src/regalloc.rs` — find the graph coloring allocator
2. Read `src/codegen/src/x86_64/stack_slot_isel.rs` line 898 (`allocate_registers` entry point)
3. For functions with >1000 instructions, switch to linear scan register allocation (O(n) instead of O(n²))
4. Implement a `LinearScanAllocator` as an alternative path
5. Add a heuristic: if `func.instructions.len() > 1000`, use linear scan; else use graph coloring
6. Verify all 40 currently-PASSING modules still PASS 20/20
7. Verify ml_kem `main` function regalloc time drops from 16s to <2s

**DoD**:
- Linear scan allocator implemented for >1000-instruction functions
- All 40 modules still PASS 20/20 on x86_64
- ml_kem compile time ≤ 20s

### Wave 3: Fix 32-bit Backend u64 Codegen

**Backends**: `arm32`, `armeb`, `hppa`, `m68k`, `sparc64`

**Approach**:
1. Compile `test_sha384_b0.vuma` for `arm32` and run with `qemu-arm-static`
2. Compare output with x86_64 output (should be identical)
3. If wrong, debug the u64 codegen in `src/codegen/src/arm32/mod.rs`:
   - u64 addition (should use ADD + ADC)
   - u64 shift (should use LSL/LSR pairs)
   - u64 rotation (should use ROR pairs)
4. Repeat for `armeb` (big-endian variant of arm32)
5. Repeat for `hppa`, `m68k`, `sparc64`
6. After each fix, re-validate all 30 modules in Domain 1 on that backend

**DoD**:
- `sha384`, `sha512`, `md5` all PASS 20/20 on `arm32`, `armeb`, `hppa`, `m68k`, `sparc64`
- Document the fix in `src/codegen/src/{arm32,armeb,hppa,m68k,sparc64}/mod.rs` comments

### Wave 4: Fix riscv32 Segfault + wasm32 print_int

**Approach for riscv32**:
1. Compile `test_sha1_b0.vuma` (simplest module) for `riscv32`
2. Run with `qemu-riscv32-static`
3. If segfault, use `gdb` to get a backtrace
4. Debug `src/codegen/src/riscv32/mod.rs` — likely a prologue/epilogue or stack alignment issue
5. Fix and re-validate

**Approach for wasm32**:
1. Compile `test_sha1_b0.vuma` for `wasm32`
2. Run with `wasmtime`
3. If `print_int` doesn't work, debug the WASM runtime in `src/codegen/src/wasm32/mod.rs`
4. The `print_int` builtin likely needs to be implemented as a WASM import
5. Fix and re-validate

**DoD**:
- `riscv32`: all simple modules (sha1, sha256, md5) PASS 20/20
- `wasm32`: all simple modules PASS 20/20

### Wave 5: Fix Remaining Backend Issues (riscv64, s390x, ppc64, ppc64le, x86_32)

**Approach**:
1. Run full validation on each backend, identify failures
2. Fix one backend at a time
3. For `x86_32`: likely u64 codegen issue (similar to 32-bit backends in Wave 3, but x86_32-specific)
4. For `riscv64`, `s390x`, `ppc64`, `ppc64le`: likely calling convention or endianness issues
5. After each fix, re-validate all modules on that backend

**DoD**:
- All 19 backends PASS ≥95% of all module combinations
- Document all codegen fixes in worklog

### Wave 6: Final Report + PR

1. Generate `DOMAIN5_REPORT.md` with:
   - SCG inliner fix summary
   - Linear scan regalloc summary
   - 32-bit u64 codegen fix summary
   - riscv32/wasm32 fix summary
   - Per-backend pass rate (all 19 backends)
2. Open PR from `agent-codegen-infra` → `main`

**DoD**: PR opened with all codegen fixes.

## Commit and Push Requirements

After each task/subtask:
```bash
cd /work/vuma
git checkout agent-codegen-infra
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  add -f <files>
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  commit -m "<type>(<scope>): <description>"
git push "https://<PAT>@github.com/pkhairkh/vuma.git" agent-codegen-infra
```

## Worklog Protocol

Before starting: read `/work/vuma/worklog.md`.
After each wave: append a section starting with `---` with Task ID `DOMAIN5-WAVE<N>`.

## Critical Rules

1. **Do NOT modify any `.vuma` module files** — those are owned by Domains 1-4
2. **Do NOT modify `scripts/validate_compact.py`** — it's read-only
3. **After each codegen fix, re-validate a sample of modules on x86_64** to ensure no regression
4. **Use `ulimit -s unlimited`** when running `compile_dump` in parallel batches (prevents stack overflow crashes)
5. **All commits must have author `pkhairkh`**
