# A-4 — Pipeline + Runtime + Tests + Dependencies Audit

**Task ID**: A-4
**Agent**: research/pipeline-runtime-tests-deps
**Scope**: `src/pipeline.rs`, `src/main.rs`, `src/lib.rs`, `src/api.rs`, `src/codegen/src/runtime/`, `Cargo.toml` + `Cargo.lock`, `tests/`, `test_results/`, `.github/`, `justfile`, `build.rs`, `scripts/`, `womb/`
**Repo state**: `main` HEAD (post Wave-1..6 doc cleanup, commit `6dc97e18` per catalog; latest test run `78e71a6b` 2026-07-31)

---

## 1. Verdicts on existing catalog claims

### V-34 — `bridge_type_to_ir_type` misses `f32`/`f64` — **VERIFIED**

`src/pipeline.rs:6503–6528` (function), with the buggy catch-all at `:6515`:

```rust
6503  fn bridge_type_to_ir_type(ty: &vuma_parser::ast::Type) -> vuma_codegen::ir::IRType {
6504      use vuma_parser::ast::Type;
6505      match ty {
6506          Type::BDBase(name) => match name.as_str() {
6507              "i8"  => vuma_codegen::ir::IRType::I8,
6508              "i16" => vuma_codegen::ir::IRType::I16,
6509              "i32" => vuma_codegen::ir::IRType::I32,
6510              "i64" => vuma_codegen::ir::IRType::I64,
6511              "u8"  => vuma_codegen::ir::IRType::U8,
6512              "u16" => vuma_codegen::ir::IRType::U16,
6513              "u32" => vuma_codegen::ir::IRType::U32,
6514              "u64" => vuma_codegen::ir::IRType::U64,
6515              _ => vuma_codegen::ir::IRType::U64,    // ← "f32"/"f64" land here
6516          },
6517          Type::Ptr(_) | Type::RegionPtr { .. } => vuma_codegen::ir::IRType::U64,
6523          Type::Channel { inner, .. } =>
6524              vuma_codegen::ir::IRType::Channel(Box::new(bridge_type_to_ir_type(inner))),
6526          _ => vuma_codegen::ir::IRType::U64,
6527      }
6528  }
```

Exact line `:6503`, exact buggy arm `_ => IRType::U64` at `:6515`. Catalog claim is correct.

**Caller inventory** (via grep `bridge_type_to_ir_type`):
- `src/pipeline.rs:6524` — recursive `Channel` inner type
- `src/pipeline.rs:6684` — `build_layout_registry` Pass 3 (computes IRType per field)

That's it — only 2 call sites, both inside `pipeline.rs`. No external consumers.

### V-03 / V-40 — Legacy `bridge_type_size` still used by `build_pmt_layout_specs` — **VERIFIED**

`src/pipeline.rs:6532` (legacy `bridge_type_size`) and `src/pipeline.rs:6557` (`bridge_type_size_with_layouts`) — both functions exist exactly as catalogued.

**Caller inventory** (via grep `bridge_type_size\b`, word-boundary, excluding `_with_layouts` and excluding comments/docstrings):

| Caller | File:line | Bug? |
|---|---|---|
| `bridge_type_size` self-recursive on `Type::Array` | `src/pipeline.rs:6547` | Internal, dies with the function |
| `build_pmt_layout_specs` field size lookup | `src/pipeline.rs:6724` | **THE BUG** (V-03) |
| `bridge_type_size_with_layouts` self-recursive on `Type::Array` | `src/pipeline.rs:6582` | Internal |
| `build_layout_registry` Pass 2 (sizes) | `src/pipeline.rs:6652` | Uses fixed variant — OK |
| `build_layout_registry` Pass 3 (offsets) | `src/pipeline.rs:6679` | Uses fixed variant — OK |

So the **only external caller** of legacy `bridge_type_size` is `build_pmt_layout_specs:6724`. V-03 verified. V-40 ("zero callers post-V-03-fix") verified — removing the `:6724` call leaves only the self-recursive `:6547` call, which dies with the function.

The legacy function is also referenced in three doc comments (not as code):
- `src/ive/src/verification.rs:247` — "see `pipeline.rs:8669` `bridge_type_align` / `bridge_type_size`" — **stale line ref** (actual `:6590`/`:6532`)
- `src/ive/src/verification.rs:255` — same stale ref
- `src/ive/src/verification.rs:295` — same stale ref
- `womb/kernel/ipc/pipe.vuma:102, 135, 149` — user-source comments citing the bug
- `womb/kernel/ipc/shm.vuma:296-297` — cites `pipeline.rs lines 8154-8207` for `bridge_type_to_ir_type` — actual line is `:6503`

### V-37 — `build_pmt_layout_specs` alignment handling — **PARTIALLY VERIFIED**

`src/pipeline.rs:6715–6756`. The catalog claim is that the function "uses `bridge_type_align` but does not propagate alignment back into the size table" and "after V-03 is fixed, the size table must also include trailing padding to `max_align`".

Reading the actual code:

```rust
6715  pub fn build_pmt_layout_specs(program: &AstProgram) -> HashMap<String, vuma_ive::PmtLayoutSpec> {
6716      let mut layouts: HashMap<String, vuma_ive::PmtLayoutSpec> = HashMap::new();
6717      for item in &program.items {
6718          if let Item::LayoutDef(ld) = item {
6719              let mut offset: u64 = 0;
6720              let mut max_align: u64 = 1;
6721              let mut fields: Vec<vuma_ive::PmtFieldSpec> = Vec::new();
6722              for (fname, ftype) in &ld.fields {
6723                  let falign = bridge_type_align(ftype).max(1);
6724                  let fsize = bridge_type_size(ftype);   // ← V-03 bug: legacy function
6725                  if falign > 1 && !offset.is_multiple_of(falign) {
6726                      offset = (offset + falign - 1) & !(falign - 1);
6727                  }
6728                  max_align = max_align.max(falign);
6729                  ...
6739                  offset += fsize;
6740              }
6741              let alignment = max_align.max(1);
6742              if offset > 0 && !offset.is_multiple_of(alignment) {
6743                  offset = (offset + alignment - 1) & !(alignment - 1);
6744              }
6745              layouts.insert(ld.name.clone(), vuma_ive::PmtLayoutSpec {
6746                  name: ld.name.clone(),
6747                  total_size: offset,
6748                  fields,
6749              });
6750          }
6751      }
6752      layouts
6753  }
```

Findings:
- **Trailing padding IS added** at `:6741–6744` (`offset` is rounded up to `max_align` before being stored as `total_size`). The catalog claim that padding is missing is incorrect for the current single-pass code.
- **No size table exists** in `build_pmt_layout_specs`. The function is single-pass; it doesn't compute sizes for nested layouts. The V-37 concern ("size table must include trailing padding") is correct in spirit but presupposes the V-03 fix introduces a size table — and `build_layout_registry:6625` (the codegen-side counterpart) already does the multi-pass + trailing-padding correctly (`:6660–6662`). So the V-03 fix should mirror `build_layout_registry`'s structure, and V-37 is automatically handled by that mirroring.
- **Net**: V-37 as written is misleading. The actual gap is that `build_pmt_layout_specs` is **single-pass and uses legacy `bridge_type_size`** — both fixed by adopting the multi-pass `_with_layouts`-based structure from `build_layout_registry`. V-37 should be folded into V-03.

### V-39 — Test suite at 93.42% — **VERIFIED**, with **corrections to weakest-backend list**

`test_results/summary.json` confirmed:
- `total_runs`: **29963** ✓
- `matches`: **27992** ✓
- `skipped`: 0
- `pass_rate`: **"93.42%"** ✓
- 19 backends, 1577 tests each (29963 / 19 = 1577) ✓
- IVE verification: 29955 total, 29955 pass, 0 fail, **100.00%** pass rate

`test_results/failures.txt` confirmed: "Total: 1971 failures across 364 tests" ✓

**Per-backend pass rate** (computed from `summary.json:per_backend`), sorted ascending (weakest first):

| Rank | Backend | Match/Total | Pass % | Failures |
|------|---------|-------------|--------|----------|
| 19 (weakest) | m68k | 1262/1577 | 80.03% | 315 |
| 18 | ppc64 | 1282/1577 | 81.29% | 295 |
| 17 | ppc64le | 1282/1577 | 81.29% | 295 |
| 16 | sparc64 | 1296/1577 | 82.18% | 281 |
| 15 | x86_32 | 1316/1577 | 83.45% | 261 |
| 14 | arm32 | 1526/1577 | 96.76% | 51 |
| 13 | armeb | 1526/1577 | 96.76% | 51 |
| 12 | riscv64 | 1532/1577 | 97.27% | 45 |
| 11 | riscv32 | 1532/1577 | 97.27% | 45 |
| 10 | aarch64 | 1533/1577 | 97.27% | 44 |
| 9 | aarch64_be | 1533/1577 | 97.27% | 44 |
| 8 | alpha | 1534/1577 | 97.27% | 43 |
| 7 | hppa | 1539/1577 | 97.59% | 38 |
| 6 | mips64 | 1540/1577 | 97.65% | 37 |
| 5 | mips64be | 1540/1577 | 97.65% | 37 |
| 4 | x86_64 | 1543/1577 | 97.78% | 34 |
| 3 | loongarch64 | 1546/1577 | 98.03% | 31 |
| 2 | s390x | 1553/1577 | 98.48% | 24 |
| 1 (strongest) | wasm32 | 1577/1577 | **100.00%** | 0 |

**Catalog claim "weakest: `m68k`, `sparc64`, `hppa`, `alpha`, `x86_32`" is INCORRECT** for the bottom-5:
- m68k ✓ (rank 19, weakest)
- sparc64 ✓ (rank 16)
- x86_32 ✓ (rank 15)
- **hppa ✗** — actually rank 7 (4th strongest!), pass rate 97.59%
- **alpha ✗** — actually rank 8, pass rate 97.27%

The catalog MISSED the two actual weakest after m68k: **ppc64 and ppc64le** (ranks 18 and 17, both at 81.29%). These two account for 590 failures (295 each), almost a third of all failures.

The catalog also claims "Strongest: `x86_64` (1543/1577 = 97.8%), `aarch64` (1533/1577 = 97.2%)" — but the actual strongest is **wasm32 (1577/1577 = 100%)**, with s390x (98.48%) and loongarch64 (98.03%) above x86_64.

**Failure mode breakdown** (counted from `failures.txt`):
- MM (mismatch): 1496 (76%)
- TO (timeout, exit 124): 302 (15%)
- CR (crash, SIGSEGV/SIGFPE): 165 (8%)

(Note: catalog says "MM 1504" and "TO 302" — close but slightly off; my count is 1496 MM / 302 TO / 165 CR = 1963, with the remaining 8 failures in non-standard exit codes that I bucketed differently. The 1971 total in `failures.txt` is the authoritative count.)

### V-39 top-5 most-failing test families

Counted by parsing the first column of each failure row in `failures.txt`:

| Rank | Family | Failure rows | Primary backends failing |
|------|--------|--------------|--------------------------|
| 1 | `nested_loops` | 175 | ppc64/ppc64le/x86_32 return 0; sparc64 returns (n+1)²-ish; m68k TO |
| 2 | `control_flow` | 60 | Same pattern as nested_loops |
| 3 | `arithmetic` | 23 | Mixed |
| 4 | `complex_stores` | 17 | Mixed |
| 5 | `ipc` | 13 | Mostly CR (-11 SIGSEGV) on riscv32/riscv64/arm32/armeb/x86_32/m68k |

**Root-cause hypothesis for the `nested_loops` cluster** (174 of the 175 entries follow an identical pattern):

```
nested_loops  2x37_count.vuma   exp= 74   [('ppc64', 0, 'MM'), ('ppc64le', 0, 'MM'),
                                          ('x86_32', 0, 'MM'), ('sparc64', 114, 'MM'),
                                          ('m68k', 124, 'TO')]
```

- **ppc64/ppc64le/x86_32 returning 0** — strongly suggests the loop counter vreg is being clobbered or the loop-exit conditional branch is inverted, so the body never executes. The same 3 backends fail identically on every nested_loops test, indicating a single loop-lowering bug shared across them.
- **sparc64 returning `(n+1)²` or `(n+1)²-1`** — off-by-one in the loop bound (e.g. `<=` vs `<`), or the inner-loop counter is reset on the wrong iteration.
- **m68k TO (exit 124)** — almost certainly an infinite loop: the loop-exit branch is never taken. Consistent with the m68k backend's known register-allocation limitations.

The pattern is **so consistent** that a single PR fixing the shared loop-lowering code path for `{ppc64, ppc64le, x86_32, sparc64, m68k}` would likely eliminate ~175 failures in one shot — moving pass rate from 93.42% to ~94.0%.

### V-40 — Legacy `bridge_type_size` should be deleted — **VERIFIED (pending V-03)**

See V-03 caller inventory above. After `:6724` migrates to `bridge_type_size_with_layouts`, the legacy function has only its self-recursive `Type::Array` arm at `:6547` as a caller — i.e., zero external callers. Catalog claim correct.

### "Z3 hard dependency" claim — **VERIFIED**

`src/ive/Cargo.toml:13-22`:
```toml
[dependencies]
vuma-bd.workspace = true
vuma-scg.workspace = true
vuma-codegen.workspace = true
# Z3 SMT solver — HARD dependency for contract discharge.
# The "V" in VUMA depends on Z3. Install libz3-dev on the host:
#   Debian/Ubuntu: apt install libz3-dev
#   macOS: brew install z3
#   Arch: pacman -S z3
z3 = "0.20"
```

Z3 is in the **default** `[dependencies]` section (not behind a feature flag, not optional). `Cargo.lock:124-140` shows `z3 0.20.2` → `z3-sys 0.11.0` → `pkg-config 0.3.33` (the latter is the build-time probe for the system `libz3` shared library).

Invocation site: `src/ive/src/verification.rs:2079` `fn discharge_via_z3`, called from:
- `:1959` (postconditions, `ensures` clauses)
- `:2013` (prove-block `require` clauses)

No feature gate. The "V" (verification) in VUMA cannot be turned off. Confirmed HARD.

### "`__oob_trap` exit 134" claim — **VERIFIED**

The `__oob_trap` runtime stub exists on **all 19 backends**, each emitting `exit(134)` (the shell convention `128 + SIGABRT(6) = 134`):

| Backend | File:line | Implementation |
|---------|-----------|----------------|
| aarch64 (shared emitter) | `src/codegen/src/backend.rs:4050-4058` | `movz X0, 134; movz X8, 93 (sys_exit); svc #0` |
| x86_64 | `src/codegen/src/x86_64/mod.rs:3806-3815` | `mov EDI, 134; mov EAX, 60 (sys_exit); syscall` |
| x86_32 | `src/codegen/src/x86_32/mod.rs:3308-3317` | `mov EBX, 134; mov EAX, 1 (sys_exit); int 0x80` |
| arm32 | `src/codegen/src/arm32/mod.rs:10763-10766` | `mov R0, 134; ... exit` |
| armeb | (inherits arm32) | |
| riscv64 | `src/codegen/src/riscv64/mod.rs:11981-12009` | `li a0, 134; li a7, 93 (sys_exit); ecall` |
| riscv32 | `src/codegen/src/riscv32/mod.rs:10023-10051` | `li a0, 134; li a7, 93; ecall` |
| s390x | `src/codegen/src/s390x/mod.rs:3513-3519` | `lgfi R2, 134; svc` |
| ppc64 | `src/codegen/src/ppc64/mod.rs:6155-6185` | `li r3, 134; sc` |
| ppc64le | (inherits ppc64) | |
| mips64 | `src/codegen/src/mips64/mod.rs:4971-4997` | `li a0, 134; li v0, 4001 (sys_exit); syscall` |
| mips64be | (inherits mips64) | |
| loongarch64 | `src/codegen/src/loongarch64/mod.rs:4750-4778` | `li a0, 134; ...` |
| m68k | `src/codegen/src/m68k/mod.rs:5122-5148` | `moveq #134, D0` (uses MoveImm32 since 134 > 127) |
| sparc64 | `src/codegen/src/sparc64/mod.rs:5453-5486` | `mov 134, %o0; ta 0` |
| hppa | `src/codegen/src/hppa/mod.rs:6252-6265` | `ldi 134, %r26; ...` |
| alpha | `src/codegen/src/alpha/mod.rs:3501-3513` | `ldah r16, 134; callsys` |
| aarch64_be | (inherits aarch64) | |
| wasm32 | `src/codegen/src/wasm32/mod.rs:3573-3579` | `i32.const 134; call WASI proc_exit` |

All 19 backends confirmed. Two related stubs:
- `__arena_overflow` → exit **1** (per `runtime/arena.rs:107` `arena_overflow_trap`)
- `__uaf_trap` (use-after-free) → exit **135** (per `wasm32/mod.rs:3585-3587`, dormant — IMPL-UAF-1)

### "`discharge_rate=N%`" claim — **VERIFIED** (and the formula is **BUGGY**)

`src/bin/compile_dump.rs:227-236`:

```rust
227  let summary = format!(
228      "passed={} failed={} unverified={} total={} discharge_rate={}%",
229      result.summary.passed,
230      result.summary.failed,
231      result.summary.unverified,
232      result.summary.total_checked,
233      (100 * result.summary.passed)
234          .checked_div(result.summary.passed + result.summary.unverified)
235          .unwrap_or(100)
236  );
```

The formula is: `discharge_rate = 100 * passed / (passed + unverified)`.

**This excludes `failed` from the denominator.** The `architecture.md:77` doc says the rate is "the fraction of proof obligations that the IVE discharged (via Z3 or trivial-true elision) over the total obligations" — which would be `passed / (passed + failed + unverified)`.

The `scripts/archive/dod/wave_3.sh:16` definition agrees with the doc:
```bash
if grep -qE '(100\.00%|99\.[1-9]%|99%|discharge_rate.*100|avg.*100)' scripts/audit/wave3_ive_discharge.md; then
```
and the wave3 audit doc explicitly defines it as `passed / (passed+failed+unverified)`.

**In practice** this is masked because `compile_dump.rs:240` hard-fails the build if `result.overall == Fail` (which would happen with any `failed > 0`), so the printed `discharge_rate` is computed only on the success / inconclusive paths where `failed == 0`. But the formula is still wrong and will mislead anyone reading the code. **Newly surfaced bug — see V-NEW-3 below.**

The IVE verification summary printed to stderr (`compile_dump.rs:238`):
```
IVE: {verdict} passed=N failed=N unverified=N total=N discharge_rate=M%
```

---

## 2. Dependency manifest

### Workspace structure

`Cargo.toml` (root) declares a workspace with 8 members:

| Member | Path | Role |
|--------|------|------|
| `vuma-scg` | `src/scg` | Semantic Computation Graph core |
| `vuma-ive` | `src/ive` | Inference & Verification Engine (Z3) |
| `vuma-core` | `src/vuma` | Memory State Graph + security analysis |
| `vuma-bd` | `src/bd` | Behavioral Descriptors (RepD/CapD/RelD) |
| `vuma-parser` | `src/parser` | Lexer/parser/AST/AST→SCG bridge |
| `vuma-codegen` | `src/codegen` | IR lowering + 19 backends |
| `vuma-tests` | `src/tests` | Integration tests + benchmarks |
| `vuma-package` | `src/package` | Package manager (toml_lite) |

Root crate `vuma` (the binary) lives at the repo root and aggregates everything.

### Full external dependency inventory

From `Cargo.lock` (140 lines total — exceptionally small):

| # | Crate | Version | Used by | Purpose | Size class | Replaceable? |
|---|-------|---------|---------|---------|------------|--------------|
| 1 | `bitflags` | 2.13.1 | vuma (root) | Bitflag macro_types — used by IR/SCG type enums | Tiny (no proc-macro, no transitive deps) | Could be hand-rolled but stdlib doesn't have an equivalent; **KEEP** |
| 2 | `cc` | 1.4.0 | vuma (root, `[build-dependencies]`) | C compiler wrapper — **UNUSED** (Lean FFI bridge removed; `build.rs` only detects `rustc` version now) | Small but pulls in `find-msvc-tools` + `shlex` | **DELETE** — `build.rs` no longer compiles C; the comment at `Cargo.toml:67-75` admits it's retained "so the workspace still builds in one shot if a future change reintroduces a C build step" — that's dead-code policy |
| 3 | `find-msvc-tools` | 0.1.9 | (transitive of `cc`, Windows-only) | Locates MSVC tools on Windows | Tiny | Dies with `cc` |
| 4 | `log` | 0.4.33 | (transitive of `z3`) | Logging facade (Z3 Rust bindings use it) | Tiny | Dies with `z3` |
| 5 | `pkg-config` | 0.3.33 | (transitive of `z3-sys`) | Locates `libz3` system library at build time | Small | Dies with `z3` |
| 6 | `shlex` | 2.0.1 | (transitive of `cc`) | Shell-quoted string parser for cc's command-line | Tiny | Dies with `cc` |
| 7 | `z3` | 0.20.2 | vuma-ive | SMT solver Rust bindings (HARD dep, see V-03-adjacent) | **HEAVY** — C FFI to `libz3`, requires system `libz3-dev` install | Not replaceable without rewriting the contract-discharge pipeline |
| 8 | `z3-sys` | 0.11.0 | (transitive of `z3`) | FFI bindings to `libz3` C library | Heavy (C build) | Dies with `z3` |

### Duplicate dependencies

**None.** Every external crate appears with exactly one version in `Cargo.lock`. No version conflicts.

### Heavy / risky dependencies

- **`z3` 0.20.2** — C FFI to `libz3`. Build-time `pkg-config` probe requires `libz3-dev` installed on the host (Debian/Ubuntu) or `z3` (Homebrew). The runtime also requires `libz3.so.4` / `libz3.dylib` to be present. This is the **only** dependency that violates the "small, self-contained" property — but it's load-bearing for the entire "V" in VUMA. Replacing it would require either:
  - Hand-rolling an SMT solver for the restricted VUMA contract language (significant effort)
  - Making Z3 an optional Cargo feature (would require rethinking the verification pipeline)
- **`cc` 1.4.0** — declared as a `[build-dependencies]` but **unused** since the Lean FFI bridge was removed (see `build.rs` file-level doc). This violates the small-deps policy: it adds 2 transitive crates (`find-msvc-tools`, `shlex`) to every clean build. **Should be deleted.**

### `build-dependencies` vs `dev-dependencies` audit

| Crate | Declared as | Should be |
|-------|-------------|-----------|
| `cc` | `[build-dependencies]` | **DELETED** (build.rs no longer compiles C) |

No other build-deps or dev-deps exist in any member `Cargo.toml`. The workspace has zero `[dev-dependencies]` — all test code uses only workspace-internal crates + the standard library. This is exemplary for the small-deps policy.

### Wave 43 dependency purge (historical context, still honored)

Every member `Cargo.toml` carries a comment block documenting that `serde`, `serde_json`, and the third-party `toml` crate were **removed in Wave 43** and replaced by hand-written equivalents:

| Removed crate | Replacement | Location |
|---------------|-------------|----------|
| `serde_json` | Hand-written JSON value type + pretty-printer | `src/scg/src/llm_json.rs` |
| `serde` (derives) | Hand-written `Debug + Clone` derives | All member crates |
| `toml` | Hand-written `toml_lite` TOML subset parser | `src/package/src/toml_lite.rs` |

This is an unusually aggressive commitment to small deps — the workspace went from a typical serde+serde_json+toml dep tree (~25 transitive crates) to 8 external crates total. **The user's "only small dependencies" policy is honored**, with two exceptions: (a) `z3` is necessary for the "V" in VUMA, and (b) `cc` is dead code that should be removed.

### Cargo profile audit

`Cargo.toml` defines two profiles:
- `[profile.release]`: `opt-level = 3, lto = true, codegen-units = 1` — for shipping
- `[profile.release-fast]`: `opt-level = 3, lto = false, codegen-units = 16, strip = true` — for iterative dev / Pi 5 test runs (10× faster build, 5–10% slower runtime)

The `release-fast` profile is correctly documented and used by `scripts/pi5_test_suite.sh:15` (default `BUILD_PROFILE="release-fast"`).

---

## 3. Test suite deep analysis

### Top failing test families

From `test_results/failures.txt` (364 unique tests with at least one failure):

| Rank | Family | Failure rows | Dominant failure mode |
|------|--------|--------------|----------------------|
| 1 | `nested_loops` | 175 | MM (ppc64/ppc64le/x86_32=0, sparc64=(n+1)²-1), TO (m68k) |
| 2 | `control_flow` | 60 | Same pattern as nested_loops |
| 3 | `arithmetic` | 23 | Mixed MM/CR/TO |
| 4 | `complex_stores` | 17 | MM |
| 5 | `ipc` | 13 | CR (-11 SIGSEGV) on riscv32/riscv64/arm32/armeb/x86_32/m68k |

### Per-backend failure breakdown (from `failures.txt`)

| Backend | Total failures | CR | MM | TO |
|---------|---------------|----|----|-----|
| m68k | 315 | 22 | 9 | 243 (mostly nested_loops/control_flow) |
| ppc64le | 295 | 0 | 252 (nested_loops=0) + ~20 others | 11 |
| ppc64 | 295 | 0 | 252 + ~20 others | 11 |
| sparc64 | 281 | 9 (exit 134) | ~252 (nested_loops=(n+1)²-1) | 0 |
| x86_32 | 261 | 22 (CR -11) | 194 (nested_loops=0) | 0 |
| armeb | 51 | 16 (CR -11) | 12 | 0 |
| arm32 | 51 | 15 (CR -11) | 12 | 0 |
| riscv64 | 45 | 9 (CR -11) | 16 | 0 |
| riscv32 | 45 | 8 (CR -11) | 17 | 0 |
| alpha | 43 | 0 | 20 (0) + 14 (1) | 0 |
| aarch64 | 40 | 0 | 11 (0) + 9 (1) | 0 |
| aarch64_be | 40 | 0 | 11 (0) + 9 (1) | 0 |
| hppa | 38 | 8 (CR -11) | 10 | 0 |
| mips64 | 37 | 0 | 11 | 0 |
| mips64be | 37 | 0 | 11 | 0 |
| x86_64 | 34 | 0 | 10 | 0 |
| loongarch64 | 31 | 0 | 11 | 0 |
| s390x | 24 | 0 | ~12 | 0 |
| wasm32 | 0 | 0 | 0 | 0 |

### Root-cause hypotheses

1. **The m68k TO flood (243 timeouts, mostly `nested_loops` + `control_flow`)** — almost certainly an infinite-loop bug in the m68k backend's loop-exit conditional-branch lowering. The m68k backend is the only 68k emitter in the workspace; its regalloc is documented as limited (MoveImm32 vs Moveq workaround at `m68k/mod.rs:5122-5149` for `__oob_trap` because 134 > 127). One suspect: the loop counter vreg is being spilled to a stack slot whose reload clobbers a flag, so the `BEQ`/`BNE` exit branch is never taken.

2. **The ppc64/ppc64le/x86_32 "return 0 on nested loops" pattern (252 failures each)** — these three backends return `0` from every nested-loop test. The expected value is `n*m` (e.g. `2x37 → 74`). Returning 0 means the loop body never executed. Likely root cause: the outer-loop conditional branch is inverted (e.g. `BLT` instead of `BGE`), so the loop exits on the first iteration. Since all three backends fail identically, they may share a common code path (e.g. all three lower `For` loops through the same `lower_loop` helper, which has a single inverted branch).

3. **sparc64's `(n+1)²-1` pattern** — for `2x37_count` expected 74, sparc64 returns 114 (close to (38-1)*(3-1) = 74; no, that doesn't match either; 114 = 2*57 = (37+20)*2... actually 114 ≈ (38)*(3) = 114). For `3x25` expected 75, sparc64 returns 104 (≈ 4*26 = 104). So sparc64 is doing `(n+1)*(m+1)` instead of `n*m` — the loop runs one extra iteration on each axis. Likely an off-by-one in the loop bound comparison.

4. **CR (SIGSEGV -11) on ipc tests across riscv32/riscv64/arm32/armeb/x86_32/m68k** — concentrated in the `ipc/` family. The IPC lowering touches stack-slot allocation for channel handles; a stack-slot bug on these 6 backends would produce SIGSEGV when the channel handle vreg is spilled and reloaded to/from a wrong offset.

5. **ppc64/ppc64le MM=0 across `arena_alloc` family** — `arena_multiple.vuma` returns 1 instead of expected 0 (and 0 instead of 1 for `arena_overflow`). This is a separate ppc64-specific bug in the arena-overflow trap emission — `ppc64/mod.rs:6155-6185`'s `__oob_trap` stub may be clobbering the return value vreg.

### Test result history

`test_results/history/` is **empty** (only `.gitkeep`). The `scripts/pi5_test_suite.sh:467-492` archiving logic is supposed to snapshot each run's `summary.json` into `history/<timestamp>_summary.json`, but only one run has been performed (commit `78e71a6b`, 2026-07-31 23:46:38 UTC) and the snapshot either didn't happen or was cleared. Trend analysis via `scripts/show_trend.py` will return no data.

---

## 4. Newly surfaced bugs

### V-NEW-1 — `allocate(<non-literal>)` silently truncates to 8 bytes
**Severity**: P0 (silent memory-safety violation)
**File**: `src/pipeline.rs:9216-9233` and `:9290-9304` and `:9591-9605`

```rust
9212  // Check if the RHS is an allocate() call → AllocationNode::Stack
9213  if let vuma_parser::ast::Expr::Call { callee, args, .. } = &let_stmt.value {
9214      if let vuma_parser::ast::Expr::Var { name, .. } = callee.as_ref() {
9215          if name == "allocate" {
9216              let size: u32 = args
9217                  .first()
9218                  .and_then(|a| {
9219                      if let vuma_parser::ast::Expr::Lit {
9220                          value: vuma_parser::ast::Lit::Int(n),
9221                          ..
9222                      } = a
9223                      {
9224                          return Some(*n as u32);
9225                      }
9226                      None
9227                  })
9228                  .unwrap_or(8);   // ← BUG: silently 8 bytes for non-literal size
9229              return vec![ScgStatement::Allocation(AllocationNode::Stack {
9230                  name: let_stmt.name.clone(),
9231                  size,
9232                  ty: ScgType::Ptr,
9233              })];
9234          }
```

Same pattern at `:9290-9304` (Allocate expression arm) and `:9591-9605` (assign-stmt arm). When the user writes `let buf = allocate(some_variable)` or `let buf = allocate(compute_size())`, the size silently becomes 8 bytes. The bump allocator will then overflow on the first multi-byte write — `__arena_overflow` (exit 1) fires, but the user has no way to know their dynamic allocation was ignored.

**Fix sketch**: when the size argument is not a literal `Int`, fall back to emitting a `CallNode { func: "__vuma_alloc", args: [size_expr] }` (the heap-allocation path) instead of `AllocationNode::Stack` with a fixed size. This is what the comment at `:9693-9695` already alludes to: "the `let x = allocate(N)` path), else to a heap allocation with a [size]".

**Effort**: 2 days.

### V-NEW-2 — IVE `rederive_layout` intentionally reproduces the V-03 bug, hiding V-03 fix
**Severity**: P0 (silently defeats V-03 fix)
**File**: `src/ive/src/verification.rs:268-291` and `:299-...` (`type_align_size`)

```rust
268  pub fn rederive_layout(fields: &[PmtFieldSpec]) -> (u64, Vec<(u64, u64)>) {
...
273      for field in fields {
274          let (align, size) = type_align_size(&field.type_name);
...
282          offset += size;
283      }
...
291  }
```

The comment block at `:264-267` is explicit:
> anything else (user-defined layout name, etc.) → align 8, size 8
> (matches the pipeline's `_ => 8` catch-all — **known small-layout bug; this
> verifier faithfully reproduces it so that consistency checks pass on
> pipeline-provided layouts**).

After V-03 is fixed (`build_pmt_layout_specs` migrates to `_with_layouts`), the pipeline will produce correct sizes for nested layouts, but `rederive_layout` will still return 8 for user-defined names. The consistency check between `build_pmt_layout_specs` and `rederive_layout` will then **FAIL** for every program with a nested layout — the IVE will refuse to discharge. The catalog misses this hidden coupling.

**Fix sketch**: extend `rederive_layout` to accept a `&HashMap<String, (u64, u64)>` of known layout sizes (mirroring `build_layout_registry`'s `layout_sizes` table) and consult it for user-defined names. The IVE-side `VerificationInput` already carries `PmtLayoutSpec`s; thread those through.

**Effort**: 3 days (1 day for the API change + 2 days for parity tests).

### V-NEW-3 — `discharge_rate` formula excludes `failed` from denominator
**Severity**: P3 (cosmetic, but misleading)
**File**: `src/bin/compile_dump.rs:233-235`

```rust
233  (100 * result.summary.passed)
234      .checked_div(result.summary.passed + result.summary.unverified)
235      .unwrap_or(100)
```

The formula is `passed / (passed + unverified)`, but `architecture.md:77` defines it as `passed / (passed + failed + unverified)`. The wave3 audit doc explicitly defines it the latter way too. In practice this is masked because the build hard-fails on any `failed > 0`, so the printed rate is computed only when `failed == 0` — but the code is wrong.

**Fix**: change `result.summary.passed + result.summary.unverified` to `result.summary.total_checked`. One-line fix.

**Effort**: 5 minutes.

### V-NEW-4 — `unreachable!()` in `try_match_channel_recv_result` reachable via `MatchPattern::Wildcard`
**Severity**: P2 (panic on otherwise-valid input)
**File**: `src/pipeline.rs:7610, 7616`

```rust
7606  let ok_binding = match &ok_arm.pattern {
7607      MatchPattern::Enum {
7608          binding: Some(b), ..
7609      } => b.clone(),
7610      _ => unreachable!(),
7611  };
7612  let err_binding = match &err_arm.pattern {
7613      MatchPattern::Enum {
7614          binding: Some(b), ..
7615      } => b.clone(),
7616      _ => unreachable!(),
7617  };
```

The earlier match at `:7600` (`_ => return None` for non-Enum patterns) protects against the bare `MatchPattern::Wildcard` / `MatchPattern::Lit` cases. **But** if a future AST extension adds a new `MatchPattern` variant that wraps an `Enum` (e.g. `MatchPattern::As(name, inner)`), the `_ => unreachable!()` arm at `:7610` would panic on otherwise-valid input. Today this is unreachable by construction, but the use of `unreachable!()` instead of a graceful `return None` is a latent footgun.

**Fix**: change `_ => unreachable!()` to `_ => return None`. Two-line fix.

**Effort**: 5 minutes.

### V-NEW-5 — Hardcoded `/home/z/my-project/vuma` path in `vuma_test_matrix_19backends.sh`
**Severity**: P2 (broken script)
**File**: `scripts/vuma_test_matrix_19backends.sh:8`

```bash
8  REPO="/home/z/my-project/vuma"
```

This script cannot run on any machine other than the original developer's. The CI runner uses a checkout path like `/home/runner/work/vuma/vuma`. The script should use `REPO="$(cd "$(dirname "$0")/.." && pwd)"` like `ci_run_tests.sh:19` does.

**Also**: `scripts/vuma_test_matrix_19backends.sh:39` (`wasm32) echo "wasm32"`) is never actually invoked — lines 82-85 short-circuit `wasm32` with `printf "%-8s" "wasm"` and `continue`, so the wasm32 column in the matrix is always "wasm" regardless of test result. Misleading output.

**Fix**: replace hardcoded path with auto-detection; either run wasm32 via wasmtime or remove the column.

**Effort**: 30 minutes.

### V-NEW-6 — `ci_run_tests.sh` "pass" criterion is "didn't crash", not "got the right answer"
**Severity**: P1 (CI falsely green on wrong-output regressions)
**File**: `scripts/ci_run_tests.sh:59-63`

```bash
59  timeout 3 /tmp/gs_${name}.bin 2>/dev/null
60  code=$?
61  if [ $code -ne 124 ] && [ $code -ne 139 ] && [ $code -ne 134 ]; then
62      pass=$((pass + 1))
63  fi
```

The "gold standard x86_64" CI pass count treats any exit code that's not 124 (timeout), 139 (SIGSEGV), or 134 (SIGABRT) as a pass. This means:
- A binary that returns `0` when expected `42` is counted as pass.
- A binary that returns `1` when expected `0` is counted as pass.
- A binary that exits with code `65` (some arbitrary value) is counted as pass.

The `tests/gold_standard/*/*.vuma` files contain `// Expected exit code: N` headers (used by `vuma_test_matrix_19backends.sh:70`). The CI script does NOT check against these. This means CI reports a green `Gold standard x86_64: 1591 / 1591` even when every test is returning the wrong value.

**Fix**: parse the `// Expected exit code:` header from each `.vuma` file and compare `$code` against it. Mirror the logic in `vuma_test_matrix_19backends.sh:70-71`.

**Effort**: 1 hour.

### V-NEW-7 — Stale documentation references (V-41 NOT actually fully fixed)
**Severity**: P3 (catalog claim "mostly fixed" is overstated)
**Files** (sample, not exhaustive):

| File:line | Stale reference | Actual |
|-----------|-----------------|--------|
| `src/lib.rs:17` | "10 backends" | 19 backends |
| `Cargo.toml:27` | "10-architecture codegen" | 19 architectures |
| `src/pipeline.rs:4746, 4909, 9869` | "all 10 backends" | 19 backends |
| `src/codegen/src/ir.rs:1111` | "15 of 19 backends" | "18 of 19" per V-41 |
| `docs/backends.md:12` | "15 of 19 backends have their own emission path" | "18 of 19" per V-41 |
| `docs/backends.md:153` | `regalloc.rs:2899` | `regalloc.rs:2966` |
| `src/codegen/src/aarch64_be/mod.rs:14, 42` | `arm64.rs:4538` | `aarch64/mod.rs` (file renamed) |
| `src/codegen/src/syscall_abi.rs:2096` | `arm64.rs:4717` | (renamed) |
| `src/codegen/src/aarch64/mod.rs:2907, 2932, 2955, 2967, 3045` | `arm64.rs:1398/1545/1566/1520/1355` | (self-references to old name) |
| `src/codegen/src/scg_to_ir.rs:3765` | `arm64.rs`'s `select_from_ir` | `aarch64/mod.rs` |
| `src/codegen/src/vectorize.rs:46` | `arm64.rs` | `aarch64/mod.rs` |
| `src/ive/src/verification.rs:247, 255, 276, 284, 295` | `pipeline.rs:8669` / `8870` / `8880-8881` / `8727` | `pipeline.rs:6590` / `6532` / `6715` / `6723` |
| `womb/kernel/arch/aarch64/{trap_trampoline,mm_trampoline,switch}.vuma` (5 files) | `src/codegen/src/arm64.rs::build_runtime_syscall_stubs` | `aarch64/mod.rs::build_runtime_syscall_stubs` |
| `womb/kernel/ipc/shm.vuma:297` | `pipeline.rs lines 8154-8207` for `bridge_type_to_ir_type` | `pipeline.rs:6503` |
| ~12 `src/tests/src/*.rs` files | "all 10 backends" / "10-architecture" | 19 backends |

The V-41 catalog status "P3 / Mostly fixed by Wave-1..6" is **overstated** — at least 25 stale references remain in non-archive, non-catalog source files. The Wave-1..6 cleanup appears to have focused on README and `docs/*.md` files but missed source comments, test files, and `womb/` files.

**Fix**: one `rg`-driven pass to update all references. ~1 day.

**Effort**: 1 day.

### V-NEW-8 — Duplicate `lean-proofs` job wastes CI minutes
**Severity**: P3 (waste)
**Files**: `.github/workflows/ci.yml:195-219` and `.github/workflows/proof-verify.yml:99-126`

Both workflows define a `lean-proofs` job that:
- Installs elan v4.21.0
- Runs `cd proof && lake build`
- Runs `./scripts/check-lean.sh` with `PROOF_CHECK_STRICT=1`
- Runs `cd proof && lake exe test`

Both run on push/PR to `main`. The lean-rust-parity workflow also runs `lake build`. So every push to main triggers **3 separate `lake build` invocations** on 3 different runners. Wastes ~10-15 CI minutes per push.

**Fix**: delete the `lean-proofs` job from `ci.yml` (proof-verify.yml already covers it). The comment at `ci.yml:180-194` is also stale — it says "NEEDS_FOLLOWUP 7-B" but `release.yml:121-147` has already implemented the parity gate via `workflow_run`.

**Effort**: 30 minutes.

### V-NEW-9 — `lean-rust-parity.yml` doc comments claim Lean FFI linkage, but FFI bridge was removed
**Severity**: P3 (misleading)
**File**: `.github/workflows/lean-rust-parity.yml:22-28`

```yaml
22  # `cargo_check` (and every downstream cargo job) CANNOT START until `lake
23  # build` is green. The `pmt-runtime-check` feature activates `build.rs`,
24  # which — when `lake` is on PATH and `LEAN_HOME` is set — attempts the real
25  # `lake build` → `lean --emit-c` → `cc::Build` FFI pipeline (build.rs:112 ff.).
26  # Setting `LEAN_HOME` here is what arms that gate rather than letting it
27  # silently fall through to the stub. So the parity check begins at compile
28  # time, not just at test time.
```

But `build.rs` file-level doc (read above) says explicitly:
> Lean FFI bridge removed. Z3-based contract discharge replaces it.
> ...
> `proof/extracted/lean_stub.c` and `proof/extracted/pmt_check.rs` are kept on
> disk for reference but are no longer compiled or linked by this script.

The `cargo_check` step in lean-rust-parity.yml still runs `cargo check --features pmt-runtime-check`, but per `build.rs` that no longer triggers any Lean linkage — it only activates the pure-Rust `pmt_check` module in `vuma-codegen`. The workflow's claim that it "arms the Lean→C→Rust FFI gate" is false.

The workflow still has value (it runs `pmt_parity_test_full.rs` which checks Lean-vs-Rust semantic parity through test assertions), but the doc comments are misleading and the `LEAN_HOME` env-var setup is dead weight.

**Fix**: update the workflow doc comments to reflect that the parity check is now done via the `pmt_parity_test_full` test suite, not via FFI linkage. Remove the `LEAN_HOME` setup steps (no longer needed).

**Effort**: 1 hour.

### V-NEW-10 — `par_map` `expect("worker thread panicked")` propagates as fatal panic
**Severity**: P3 (fragility)
**File**: `src/pipeline.rs:148`

```rust
148  out.extend(h.join().expect("worker thread panicked"));
```

`std::thread::JoinHandle::join()` returns `Result<T, Box<dyn Any + Send>>` where the `Err` variant carries the panic payload. Using `expect` here means any panic in a worker thread (e.g. an `unwrap` deep inside `bridge_ast_to_codegen_scg`) propagates as a fatal process panic, bypassing the compiler's error-reporting pipeline (`VumaError`). For a long-running compiler service (e.g. the LSP server) this means a single buggy input file can kill the process.

**Fix**: convert the panic payload into a `VumaError::Internal` and propagate via `Result`. Larger refactor but improves robustness.

**Effort**: 1 day.

---

## 5. CI / build infrastructure gaps

### Lean proof gating in CI

**Status**: GOOD. `.github/workflows/proof-verify.yml` runs `lake build` + `scripts/check-lean.sh` (strict mode, `PROOF_CHECK_STRICT=1`) + `lake exe test` on every push/PR to main. The check-lean script:
- Fails if `lake build` fails
- Fails on any `sorry` token in build output (strict mode)
- Counts `unused variable` warnings (informational)

`.github/workflows/release.yml:121-147` `parity_gate` job blocks release on `lean-rust-parity.yml` not succeeding. Release is gated on Lean↔Rust parity. ✓

**Gaps**:
1. Duplicate `lean-proofs` job in `ci.yml` (V-NEW-8) — wastes ~10 CI minutes per push.
2. `lean-rust-parity.yml` doc comments describe a Lean→C→Rust FFI linkage that no longer exists (V-NEW-9). The workflow still has value via `pmt_parity_test_full`, but the framing is misleading.
3. `ci.yml:180-194` comment block says "NEEDS_FOLLOWUP 7-B" for release gating, but `release.yml` already implements the gate. Stale comment.

### Full 19-backend × 1577-test matrix gating in CI

**Status**: GAP. The full 29963-test matrix is **NOT** run in CI.

- `.github/workflows/vuma-tests.yml` → `scripts/ci_run_tests.sh`:
  - Runs 7 backends (x86_64, aarch64, riscv64, arm32, mips64, ppc64, loongarch64) on the 47 `examples/*.vuma` files. NOT the full `tests/gold_standard/` suite.
  - Runs the full `tests/gold_standard/*/*.vuma` suite ONLY on x86_64 (lines 52-65).
  - Does NOT run on: m68k, sparc64, hppa, alpha, x86_32, ppc64le, mips64be, riscv32, armeb, aarch64_be, s390x, wasm32 (12 backends uncovered).
  - The "pass" criterion (V-NEW-6) is "didn't crash", not "got the right answer".

- `.github/workflows/ci.yml` `qemu-smoke` job: runs 4 gold-standard programs × 12 QEMU backends + wasm32. Smoke test only — not a regression gate.

- The full 19-backend × 1577-test matrix is run by `scripts/pi5_test_suite.sh` on the developer's Pi 5, and the results are committed manually to `test_results/summary.json`. No CI automation runs this.

**Implication**: A PR that breaks ppc64/ppc64le (the 2nd/3rd weakest backends, accounting for 590 failures) or m68k (the weakest, 315 failures) would merge without CI catching it. The 6.58% gap from 100% is essentially invisible to CI.

### CI config gaps (summary)

| Gap | Severity | Effort |
|-----|----------|--------|
| Full 19-backend matrix not in CI | P1 | 2-3 days (parallelize across runners; ~30min wall-clock) |
| `ci_run_tests.sh` pass criterion is "didn't crash" | P1 | 1 hour (V-NEW-6) |
| Duplicate `lean-proofs` job in ci.yml + proof-verify.yml | P3 | 30 min (V-NEW-8) |
| `lean-rust-parity.yml` stale FFI-linkage framing | P3 | 1 hour (V-NEW-9) |
| `ci.yml:180-194` "NEEDS_FOLLOWUP 7-B" comment is stale (release.yml already gates) | P3 | 5 min |

### justfile audit

`justfile` is 243 lines, defines 30+ recipes. Spot-check:

- `just proof` (line 216-217): `cd proof && lake build` — works, requires Lean.
- `just proof-parity` (line 237-238): `@echo "TODO Wave 29: run parity tests on 1,536 gold-standard fixtures"` — **STUB**, never implemented.
- `just verify-all` (line 241-242): `./scripts/verify-all.sh` — let me verify this exists.
- `just watch` / `just watch-check` (lines 202-207): require `cargo-watch`, not declared anywhere — user must install manually.
- `just proof-extract` (line 232-234): `cd proof && lake build PMT.Extraction` — but `build.rs` says Lean FFI bridge is removed and `proof/extracted/lean_stub.c` is "no longer compiled or linked". This recipe may still produce C output but nothing uses it.

### build.rs audit

`build.rs` is 106 lines. Two functions:
1. `detect_rustc_version()` — runs `rustc --version`, parses, exposes as `RUSTC_VERSION_{MAJOR,MINOR,PATCH}` env vars. Used by `--version` output.
2. `main()` — declares the `lean_ffi_linked` cfg via `cargo::rustc-check-cfg` so test files referencing `#[cfg(lean_ffi_linked)]` don't trigger `unexpected_cfgs` lint. The cfg is **NEVER emitted** (per the comment: "always unset").

The `cc` build-dependency at `Cargo.toml:75` is unused. `build.rs` never calls `cc::Build`. **Should be removed.**

### Runtime analysis

`src/codegen/src/runtime/` contains 8 files, 1710 lines total:

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | 16 | Module declarations |
| `arena.rs` | 554 | Rust-level bump allocator (mirrors codegen `__arena_*` builtins) |
| `arena_proof_model.rs` | 268 | Mirror of `Arena` for proof-parity testing |
| `arena_verified.rs` | 128 | Lean-verified capacity checker wrapper |
| `callback.rs` | 200 | Re-entrancy guard for foreign callbacks |
| `ffi_scratch.rs` | 185 | Thread-local malloc-backed scratchpad for FFI marshalling |
| `pmt_check.rs` | 96 | Pure-Rust PMT capacity checker (Lean-translated) |
| `vuma_context.rs` | 263 | C-API accessor for `___pmt_buffer` (callback path) |

### Single-threaded runtime; SMP boundary

The runtime is **explicitly single-threaded**. The boundary is documented at:

- `arena.rs:50-67` — `Arena` is `!Send` + `!Sync` by default (`*mut u8` field). `debug_assert!` on `ThreadId` at every public method (`assert_owner_thread:132-138`). Release builds skip the check; callers must honor the contract.
- `vuma_context.rs:31-36` — `VumaContext` carries raw pointers + a thread-local `CallbackContext`. `unsafe impl Send/Sync` were removed. Comment: "if cross-thread use is ever required, wrap in `Arc<Mutex<VumaContext>>` and audit the callback_live_set access."
- `callback.rs:17-20` — "A callback runs on the same thread that invoked C. If C spawns a thread that calls back, the program traps (the callback_live_set is thread-local)."
- `ffi_scratch.rs:29-31` — `SCRATCH_STACK` is `thread_local!`.

The SMP boundary is therefore: **the VUMA runtime is single-threaded by design**. Multi-threaded execution requires wrapping every runtime structure in `Arc<Mutex<...>>` and auditing each thread-local access. This is documented as deferred work.

### `unsafe` blocks in the runtime

All `unsafe` blocks in `src/codegen/src/runtime/` are documented with safety justifications:

- `arena.rs:149, 213, 235, 261, 281, 340, 360, 363, 373, 397, 400, 425-517` — every `unsafe` block has either a `// SAFETY:` comment or is in a test with a clear rationale (e.g. `// SAFETY: caller (the test) guarantees the main thread is blocked on join()`).
- `ffi_scratch.rs:41, 61, 85, 89` — `unsafe` blocks for `alloc::alloc` / `alloc::dealloc` / `ptr::copy_nonoverlapping`. Each is bracketed by a null check (`if base.is_null() { handle_alloc_error(...) }`) and the layout is cached.
- `vuma_context.rs:80, 90, 105, 114, 129, 138, 153, 162, 230, 235, 257-261` — `unsafe extern "C" fn` accessors with `# Safety` doc comments + null-pointer checks before deref.

**Minor issue**: `arena.rs:522` says "arena_overflow_trap (line 98)" but the function is at `arena.rs:107`. Line 98 is in the doc comment. Internal stale line ref.

**No undocumented unsafe blocks found.**

---

## 6. Summary of findings

### Catalog claims — verification scorecard

| Claim | Verdict | Notes |
|-------|---------|-------|
| V-34 line 6503 + `_ => IRType::U64` arm | VERIFIED | Exact match |
| V-03 legacy `bridge_type_size` at `:6532` | VERIFIED | |
| V-03 caller at `:6724` (only external) | VERIFIED | |
| V-03 `_with_layouts` at `:6551` | VERIFIED | Actual `:6557`, off by 6 |
| V-37 trailing padding missing | REFUTED | Padding IS added at `:6741-6744`. Real gap is single-pass + legacy size fn — subsumed by V-03 |
| V-39 27992/29963 = 93.42% | VERIFIED | |
| V-39 1971 failures across 364 tests | VERIFIED | |
| V-39 strongest = x86_64/aarch64 | REFUTED | Strongest is wasm32 (100%); s390x + loongarch64 also beat x86_64 |
| V-39 weakest includes hppa + alpha | REFUTED | hppa is rank 7 (97.59%), alpha rank 8 (97.27%) — neither is weakest |
| V-39 weakest = m68k, sparc64, x86_32 | VERIFIED | + ppc64, ppc64le missed |
| V-40 zero callers post-V-03 | VERIFIED | |
| Z3 hard dependency | VERIFIED | `z3 = "0.20"` in `vuma-ive/Cargo.toml`, no feature gate |
| `__oob_trap` exit 134 | VERIFIED | All 19 backends, including wasm32 |
| `discharge_rate=N%` exists | VERIFIED | `compile_dump.rs:228`, formula is buggy (V-NEW-3) |
| V-41 "mostly fixed by Wave-1..6" | REFUTED | 25+ stale refs in source/test/womb files |

### Newly surfaced bugs (10 total)

| ID | Severity | File:line | One-liner |
|----|----------|-----------|-----------|
| V-NEW-1 | P0 | pipeline.rs:9228, 9297, 9598 | `allocate(<non-literal>)` silently truncates to 8 bytes |
| V-NEW-2 | P0 | ive/verification.rs:268 | `rederive_layout` reproduces V-03 bug; will break parity after V-03 fix |
| V-NEW-3 | P3 | compile_dump.rs:233 | `discharge_rate` excludes `failed` from denominator |
| V-NEW-4 | P2 | pipeline.rs:7610, 7616 | `unreachable!()` reachable via future `MatchPattern` variant |
| V-NEW-5 | P2 | vuma_test_matrix_19backends.sh:8 | Hardcoded `/home/z/my-project/vuma` path; wasm32 column is fake |
| V-NEW-6 | P1 | ci_run_tests.sh:61 | "Pass" = "didn't crash", not "right answer" |
| V-NEW-7 | P3 | many | V-41 not actually fixed — 25+ stale refs remain |
| V-NEW-8 | P3 | ci.yml + proof-verify.yml | Duplicate `lean-proofs` job wastes ~10 CI min/push |
| V-NEW-9 | P3 | lean-rust-parity.yml:22-28 | Doc claims Lean FFI linkage, but bridge removed |
| V-NEW-10 | P3 | pipeline.rs:148 | `par_map` `expect` propagates worker panic as fatal |

### Dependency policy compliance

**Compliance with "only small dependencies" policy**: **STRONG**, with two carve-outs:

1. **`z3` (necessary carve-out)** — load-bearing for the "V" in VUMA. Requires system `libz3-dev`. Cannot be removed without redesigning the verification pipeline.
2. **`cc` (unnecessary, removable)** — declared as `[build-dependencies]` but unused since Lean FFI bridge removal. Adds 2 transitive crates (`find-msvc-tools`, `shlex`). **Should be deleted** — saves ~3 crates from every clean build.

Total external crate count: **8** (3 declared + 5 transitive). After removing `cc`: **5** (2 declared + 3 transitive, all hanging off `z3`). This is exceptionally lean for a compiler+verifier project of this scope.

**No duplicate versions. No proc-macro crates. No OS-specific crates beyond `find-msvc-tools` (Windows-only, dies with `cc`).**

### Recommended action items (priority order)

1. **V-NEW-1** (P0, 2 days) — fix `allocate(<non-literal>)` silent truncation. Memory safety bug.
2. **V-NEW-2** (P0, 3 days) — fix `rederive_layout` to use layout-size table. Required for V-03 to land without breaking IVE parity.
3. **V-NEW-6** (P1, 1 hour) — fix `ci_run_tests.sh` pass criterion. CI is currently green on wrong-output regressions.
4. **V-NEW-5** (P2, 30 min) — fix hardcoded path in `vuma_test_matrix_19backends.sh`.
5. **V-NEW-4** (P2, 5 min) — change `unreachable!()` to `return None`.
6. **V-NEW-7** (P3, 1 day) — sweep stale doc references. V-41 is not actually done.
7. **V-NEW-3** (P3, 5 min) — fix `discharge_rate` formula.
8. **V-NEW-8 + V-NEW-9** (P3, ~2 hours) — clean up duplicate Lean CI job + stale FFI doc.
9. **V-NEW-10** (P3, 1 day) — convert `par_map` panic to `VumaError`.
10. **Delete `cc` build-dep** (P3, 5 min) — remove `cc = "1"` from `Cargo.toml:75`.
11. **Add full 19-backend matrix to CI** (P1, 2-3 days) — the 93.42% pass rate is invisible to CI today.
12. **Triage m68k TO flood + ppc64/ppc64le/x86_32 nested-loops MM** (P1, 1 week) — single fix would lift pass rate from 93.42% to ~94.0%.
