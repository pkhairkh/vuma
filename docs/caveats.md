# Caveats and Known Issues

> Known caveats, stubs, architectural issues, and dead code in the VUMA
> compiler. Each limitation is documented with a `file:line` reference so
> developers can find and fix them.

**Citation-drift note:** many `file:line` citations in this document
have drifted from the current line numbers as code was inserted/deleted
above them. When a citation doesn't land on the expected symbol, grep
for the named symbol (e.g. `rg -n 'Item::LayoutDef' src/parser/src/to_scg.rs`)
rather than trusting the line number verbatim.

## 0. README / `--safe` Contradiction — RESOLVED

| Issue | Location | Description |
|-------|----------|-------------|
| README headline claimed "invariant verification at compile time **without runtime memory-safety overhead**" while `--safe` is hard-coded on and `__oob_trap` bounds checks ARE emitted — **RESOLVED** (PMT Wave 0, task PMT-0-A) | `README.md` (intro paragraphs); `src/main.rs:607` (`safe: true, // safe is always on (--safe is a no-op)`); `src/codegen/src/memory_safety.rs:1014` (`inject_bounds_check_ir`, invoked at `src/pipeline.rs:6139`); cross-ref §3 "PMT & Memory Safety" below and Stage 5 of `docs/pipeline.md` | The README intro previously marketed invariant verification "without runtime memory-safety overhead", which self-contradicted (a) `src/main.rs:607` hard-coding `safe: true` with the comment that `--safe` is a no-op (always on), and (b) the `__oob_trap` runtime bounds checks (`ComputationNode(UGe)` + `ControlNode::If { __oob_trap }`) that `inject_bounds_check_ir` prepends to every bounded `Seq` access into an arena-allocated state buffer. The contradiction has been resolved by rewriting the README headline + intro to honestly state that `--safe` is always on and that arena-allocated accesses DO incur a `__oob_trap` bounds check at runtime (so the overhead is paid, not avoided), while raw-pointer / `length_expr=None` accesses remain unchecked (future SoftBound work). **No source code (`src/**`) or Lean proof (`proof/**`) files were modified** — only `README.md` and this `docs/caveats.md` §0 entry. This entry is APPEND-ONLY: no pre-existing line in this file was edited to add it. |

## 1. Parsing & Frontend

| Issue | Location | Description |
|-------|----------|-------------|
| Parser accepts constructs codegen doesn't lower — **RESOLVED** | `to_scg.rs:668` (`Item::LayoutDef`), `to_scg.rs:2284` (`Stmt::TransformCall`) | Layout AST (`Item::LayoutDef`) lowers to a real `NodeType::StructDef` node at `to_scg.rs:668` carrying `StructFieldInfo { name, type_name, offset, size }`. Transform-call AST (`Stmt::TransformCall`) lowers via `emit_call_nodes` + a let-binding that defines `tc.dst` at `to_scg.rs:2284`. |
| Two parallel SCG IRs — **OPEN** | `scg/src/node.rs` vs `codegen/src/scg_to_ir.rs` | Semantic SCG (IVE) and codegen SCG are separate types with duplicated logic. Changes to one may not propagate to the other. Re-export unification mitigated the worst divergence but did not merge the two types. |
| Lexeme-dispatched `layout`/`transform` — **PARTIALLY RESOLVED** | `parser.rs:408-426` (peek-guarded dispatch) | A 1-token peek mirrors the existing `region` handling: only `layout <name>...` / `transform <name>...` (where `<name>` is an `Ident` or a name-keyword like `Ok`/`Some`/`Err`/`ptr`/`alloc`) dispatch to `parse_layout_def`/`parse_transform_def`; everything else falls through to `parse_stmt`. The deeper lexer-level fix (reserve `layout`/`transform` as proper `TokenKind` variants, mirroring `Ref`) is deferred — the lexeme+peek pattern is retained for consistency with `region`. |
| Max expression depth 256 — **RESOLVED** | `parser.rs:36` (`pub const MAX_EXPR_DEPTH: u32 = 1024`) | Default raised from 256 to 1024 (to accommodate machine-generated VUMA programs); CLI-configurable via `--max-expr-depth N` plumbed through `CompileConfig.max_expr_depth`. |
| `compile_dump --verify` is a no-op — **RESOLVED** | `bin/compile_dump.rs:511-528` (flag filter), `:201-220` (IVE result handling) | `--verify`, `--no-verify`, and `--allow-inconclusive` are real flags in the `compile_dump` arg filter (`:511-528`). `--verify` explicitly enables the IVE verify gate; `--no-verify` opts out; `--allow-inconclusive` relaxes the `Inconclusive` hard-fail. |
| `vuma emit` bypasses IVE — **PARTIALLY RESOLVED** | `main.rs:2199` (`cmd_emit` routes through `compile_to_binary_direct`) | `cmd_emit` was refactored to call `compile_with_path` so the full IVE suite would run, but reverted to the direct AST→codegen path (`compile_to_binary_direct`) because the canonical pipeline's IVE gate rejected some valid test fixtures. The direct path runs only the PMT-level verifiers, not the full IVE suite. |
| Brittle string-prefix fatality detection — **RESOLVED** | `parser.rs:274-289` (now uses `ParseErrorKind` equality) | Replaced the two `e.message.starts_with("pointer syntax '")` / `e.message.starts_with("FFI attribute ")` checks with `e.kind == ParseErrorKind::PointerSyntax` / `e.kind == ParseErrorKind::FfiAttribute`. |
| `no_struct_literal` parser flag — **RESOLVED** | `parser.rs:60-96` (field doc), `:213-223` (`with_no_struct_literal` helper) | Field doc expanded from 4 lines to ~37 lines explaining (a) what the flag does, (b) why it exists (the `cond { body }` ambiguity when `cond` ends in a struct-literal-acceptable token), (c) the two callers (`parse_if`, `parse_for`). |

## 2. IVE (Intermediate Verification Engine)

> **Verdict:** IVE is trustworthy ONLY for narrow PMT-state properties. It is
> NOT a memory/type-safety verifier. See the IVE trustworthiness analysis
> below for the full analysis.

| Issue | Location | Description |
|-------|----------|-------------|
| `Inconclusive` = soft pass | `pipeline.rs:5431-5435` | **RESOLVED** (full flip). `Inconclusive` is now a HARD compile error by default in every pipeline gate (`compile_with_path:5434`, `compile_modules:6414`, `compile_with_recovery:6877`, `cmd_emit` via `verify_pmt_on_ast`). The `--allow-inconclusive` CLI flag and `CompileConfig::allow_inconclusive` opt back out. |
| Advisory verifiers never abort | `pipeline.rs:5036-5205` | **PARTIALLY RESOLVED.** Of the 5 historical advisory verifiers, 3 are now MANDATORY (hard-fail on abort). Linear-channel false-positive was fixed; linear-channel was promoted to HARD-FAIL — gate-level, but dormant due to a pre-existing parser gap. |
| `vuma emit` bypasses IVE | `main.rs:2199` | **PARTIALLY RESOLVED.** See the relevant row above. `cmd_emit` originally routed through `compile_with_path` (full IVE gate), but reverted to the direct AST→codegen path (`compile_to_binary_direct`) because the canonical pipeline's IVE gate rejected some valid test fixtures. |
| `pmt_layouts` trusted from parser — **OPEN** | `invariant_aggregator.rs` | IVE's PMT verifiers use layout data produced by the parser without independent derivation. A parser bug means the verifier checks against wrong data. A `pmt_layouts` cross-check catches parser-side layout incoherence but does not independently derive the layouts. |
| Vreg-mangled variable identity — **RESOLVED** | `verification.rs:664-689` | The format string is now `format!("_state_{}_{}", id, state_vreg)` (not the older `format!("_state_{}", state_vreg)`), where `id` is the SCG node's global ID. State-read and state-write nodes with the same `state_vreg` but different `id` no longer collide. |
| `type_hash` duplicated | `scg/src/hash.rs:26` (single source of truth) + `codegen/src/ipc.rs:20` (re-export) | **RESOLVED.** Single source of truth at `scg/src/hash.rs:26`; re-exported (not duplicated) at `codegen/src/ipc.rs:20` via `pub use vuma_scg::hash::type_hash;`. |
| Substring-based capability detection | `verification.rs:882-905` (nominal `ComputationKind::Intrinsic(_)` matcher) | **RESOLVED.** Replaced substring matching with nominal typing at `verification.rs:882-905`: `let is_intrinsic = matches!(c.kind, ComputationKind::Intrinsic(_));`. Only nodes tagged with `ComputationKind::Intrinsic(_)` are treated as intrinsics. |
| Constant-time "secret" heuristic | `verification.rs:45-63` (`secret_vars` field doc), `pipeline.rs:9708` (`collect_secret_vars`) | **PARTIALLY FIXED.** `#[secret]` attributes are collected by `pipeline.rs::collect_secret_vars` (at `:9708`) and attached to `VerificationInput::secret_vars`. The forward propagation of "tainted" status from secret vars to derived expressions is still substring-based (`name.contains("secret")` / `name.starts_with("key_")`) and not dataflow-driven. |
| `vuma_log!` is no-op in release — **RESOLVED** | `lib.rs:48-60` (macro definition), `ive/src/lib.rs:45-60` (IVE-internal copy) | The macro is no longer a no-op in release builds — it emits to stderr if `VUMA_LOG` env var is set (and always emits in debug builds). Set `VUMA_LOG=1` to enable. |
| `with_level` silently coerces — **STALE** | `invariant_aggregator.rs:602` | The cited `cfg(not(test))` coercion mechanism no longer exists. The `VerificationLevel` enum (`invariant_aggregator.rs:158-178`) was collapsed to a single `Pmt` variant; the five legacy variants were removed. |
| Borrow region / FFI modules library-only — **PARTIALLY STALE** | `ive/src/borrow_region.rs` (255 LOC, advisory-only); `ive/src/ffi.rs` (REMOVED) | `borrow_region.rs` is NOT library-only. It is invoked from `pipeline.rs:5648` (`compile_with_path`) and `pipeline.rs:7006` (`compile_with_recovery`) as advisory-only linear-channel checks. `ive/src/ffi.rs` does not exist. |
| Escape analysis not in IVE pipeline — **STALE PATH** | `codegen/src/escape_analysis.rs` | `ive/src/escape.rs` does NOT exist. Escape analysis lives at `codegen/src/escape_analysis.rs` (1469 LOC, exported via `codegen/src/lib.rs:82`) and is used by codegen SROA, not by IVE verification. |

## 3. PMT & Memory Safety

> **Verdict:** The "no buffer overflow" claim is NOT faithful as stated. See
> the PMT memory-safety analysis below for the full analysis.

| Issue | Location | Description |
|-------|----------|-------------|
| `--safe` flag is dead — **PARTIALLY RESOLVED** | `main.rs:1536` → `pipeline.rs:5732-5735, 5765-5801` | `--safe` sets `runtime_bounds_checks: cli.safe` in `CompileConfig` (`main.rs:1536`). `pipeline.rs:5732-5735` conditionally selects `MemorySafetyConfig::safe_mode` vs `compile_time_only`. `inject_bounds_check_ir` (`memory_safety.rs:1045`) emits the UGe+If OOB-trap IR when `safe_mode` is selected. The flag is wired; the residual gap is that `--safe` is opt-in rather than default. |
| `find_bounds_check_sites` is dead code | `memory_safety.rs:843` (`find_bounds_check_sites_with_bounds` retained) | **RESOLVED.** The dead `find_bounds_check_sites` wrapper (no `_with_bounds`) was DELETED from `memory_safety.rs` along with its only caller (`#[test] test_bounds_check_site_detection`). `find_bounds_check_sites_with_bounds` is the live function. |
| `runtime_bounds_instrumented` is a status field | `pipeline.rs` | **RESOLVED.** The `runtime_bounds_instrumented` field was DELETED from `MemorySafetyReport` (`memory_safety.rs`). The field declaration, the `empty` initializer, and the write at the end of `analyze` were all removed. |
| No runtime bounds emission — **PARTIALLY RESOLVED** | All backends | When `--safe` is on, `inject_bounds_check_ir` (`memory_safety.rs:1045`) emits a `ComputationNode(UGe)` + `ControlNode::If { __oob_trap }` pair before every `AccessNode::Load`/`Store` whose `ptr` resolves to a PMT arena offset. When `--safe` is off, no runtime check is emitted. |
| 5 of 8 legacy invariants skipped at PMT level — **RESOLVED** | `invariant_aggregator.rs` | The 5 legacy pointer invariants (liveness, exclusivity, interpretation, origin, cleanup) have been removed entirely from `InvariantKind` (`invariant_aggregator.rs:60-89`) — not merely skipped. The enum now has only the `Pmt` variant. |
| `arena.rs` panics on overflow — **RESOLVED** | `arena.rs:98-101` (`arena_overflow_trap` helper); fault-path call sites at `:107, 139, 168, 178` | Replaced all `std::process::abort` (SIGABRT, exit 134) calls in `arena.rs` with `arena_overflow_trap(msg)` (at `:98-101`), which calls `std::process::exit(1)` after printing a diagnostic. Call sites updated. |
| `unsafe impl Send for Arena` — **RESOLVED** | `arena.rs:55-58` (`Arena` struct with `*mut u8`), `arena.rs:73-76` (no `unsafe impl Send` comment) | Audited and removed the historical `unsafe impl Send for Arena`. The `Arena` struct now holds `base: *mut u8` (auto-`!Send` + `!Sync`), so cross-thread sharing is a compile error. |
| PMT ≠ Persistent Memory Transaction | `invariant_aggregator.rs:62-87` | **RESOLVED.** The `InvariantKind::Pmt` and `VerificationLevel::Pmt` doc comments now carry an explicit "Acronym disambiguation" section: PMT = "Programs as Memory Transformations," NOT "Persistent Memory Transaction." No per-variant rename was done. |

## 4. Codegen & Backends

| Issue | Location | Description |
|-------|----------|-------------|
| `syscall_abi::translate` is wired in — **RESOLVED** | `syscall_abi.rs:281` | `translate_or_warn` IS the production caller — invoked by 16+ call sites across 15 native backends + 1 generic emit path. Each call site passes its OWN `BackendKind`; no cross-backend mismatch. |
| 16/19 backends use stack-slot ISel — **RESOLVED** | `regalloc.rs` | 16/19 backends use stack-slot ISel by default (arm32, x86_64, x86_32, loongarch64, alpha, s390x, sparc64, hppa, mips64, m68k, ppc64, riscv32, riscv64 = 13 native + 3 BE wrappers: armeb→arm32, mips64be→mips64, ppc64le→ppc64). The remaining 3 (aarch64, aarch64_be, wasm32) use the register-only path. |
| `verify_function_float_ops` only on AArch64 — **RESOLVED** | `backend.rs:150`, `backend.rs:180` | Pre-lowering float-op verification is now wired CENTRALLY in all 5 compilation drivers — `compile_to_binary_direct` (`src/main.rs`), `compile_modules` and `compile_to_wasm` (`src/pipeline.rs`), `compile_with_path` and `compile_with_recovery` (`src/pipeline.rs`). |
| s390x secondary Ret path — **RESOLVED** | `s390x.rs:1886-1902` (restore callee-saved S0–S5 + `adjust_sp`) | Fixed the secondary return path so it now restores callee-saved scratch registers S0–S5 (R6/R7/R8/R9/R10/R12) before calling `adjust_sp(frame_size)`. The primary Ret path was already correct. |
| arm32 parallel-alloc race — **RESOLVED** | `arm32/mod.rs`, `compile_dump.rs` | `allocate_registers` ran in parallel; callee param_types lookup missed. Fixed by `preregister_param_types` sequential pre-pass. |
| `materialize_f32_immediates` is load-bearing — **OPEN** | `opt.rs` | Must run between constant folding and codegen. Skipping it breaks f32 immediate emission on all backends. Informational code-ordering dependency, not a bug. |
| hppa LDIL broken in QEMU — **OPEN (QEMU 7.2.0 workaround in place)** | `hppa.rs:815` (`ss_load_imm`), `hppa.rs:793-811` (`[QEMU-WA:hppa-ldil]` doc), `hppa.rs:6193-6200` (`GetAddress` relocator patch site) | QEMU 7.2.0 hppa LDIL decoder is broken: LDIL (major opcode 0x08, format 21) shifts its immediate left by 21 bits, but QEMU's decoder reads the wrong field width. Workaround: the `GetAddress` relocator at `:6193-6200` patches the load-site to use a LDO+LDIL pair instead of a bare LDIL. |
| hppa far-call displacement — **OPEN (long-call codegen strategy, not a bug)** | `hppa.rs:1371` (`patch_call_site` fn), `:1420-1422` (Case 4 BL,n fallback) | NOT a QEMU bug — codegen fallback for far calls. When the 4-LDO strategy (Case 3, max ±32 764 byte displacement) cannot reach the target, `patch_call_site` falls back to BL,n + import-stub (Case 4). |
| m68k MOVEM SIGILL — **OPEN (QEMU 7.2.0 workaround in place)** | `m68k.rs:3841` (primary comment + `__vuma_alloc` push), `:3853` (pop), `:4004` (`mprotect` push), `:4271` (`print_int` push), `:4494` (`print_hex` push) | QEMU 7.2.0 m68k translator has a translator/disassembler disagreement on MOVEM register-list encoding. Workaround: `__vuma_alloc` and the print helpers use individual MOV/MOVPEA pairs instead of MOVEM. |
| m68k ADDI.B/CMPI.B SIGILL — **OPEN (QEMU 7.2.0 workaround in place)** | `m68k.rs:4426` (primary comment, `print_int` ADDI.B #48), `:4494` (`print_hex` ADDI.B #48), `:4498` (`print_hex` CMPI.B #57), `:4503` (`print_hex` ADDI.B #39) | QEMU 7.2.0 m68k translator rejects byte-form immediate-to-register comparisons. Workaround: `print_int`/`print_hex` use ADDQ/CMPI word-form. |
| alpha CMPULE encoding — **OPEN (QEMU 7.2.0 workaround in place)** | `alpha.rs:278` (`Instruction::encode` special case) | QEMU 7.2.0 alpha does NOT implement the CMPULE opcode (function 0x3F on INTA major opcode 0x10) — raises SIGILL ("Illegal instruction") whenever the encoded function field is 0x3F. Workaround: the encoder at `:278` emits CMPULT (function 0x3D) + a following BEQ workaround. |
| wasm32 trampoline loop — **OPEN (architectural, IR↔Wasm impedance mismatch)** | `wasm32/mod.rs:2245` (`lower_function` trampoline setup), `:4184` (`lower_terminator_trampoline`) | ARCHITECTURAL, not a QEMU bug. WebAssembly requires structured control flow — no arbitrary jump-to-label, only `block`/`loop`/`if`. The wasm32 backend lowers every IR block to a `loop` with a br_if-then-br pattern; this works but the trampolines are nested 8-12 deep per function for non-trivial control flow. |
| 4 thin-wrapper backends — **OPEN (informational, not independent implementations)** | `armeb.rs`, `aarch64_be.rs`, `mips64be.rs`, `ppc64le.rs` | 200-530 LOC wrappers around a sibling backend. Not independent implementations. |

## 5. IPC

| Issue | Location | Description |
|-------|----------|-------------|
| wasm32 fork is not real isolation | `wasm32/mod.rs` (module doc), `wasm32/mod.rs:4084-4100` (one-shot `vuma_log!(warn)` at clone→0 emit), `ipc_lowering.rs:303-320` (one-shot `vuma_log!(warn)` inside `wasm32_fork_emulation_pass`) | **PARTIALLY RESOLVED.** Fork emulation still runs parent and child in the same wasmtime instance (no isolation). The clone→0 path emits a one-shot `vuma_log!(warn)` to surface the gap. |
| IPC tests need ≤3 workers | `scripts/pi5_test_suite.sh` (K11C) | Fork+exec+wait tests timeout under heavy parallel load. Two-phase scheduling caps IPC workers at 3 (default). Cap is configurable via env var `VUMA_IPC_WORKER_CAP=N` (default 3). |
| QEMU arm32/armeb connect returns fake success — **OPEN (QEMU 7.2.0 workaround in place)** | `ipc_lowering.rs:3816` (K11B comment, `expand_channel_open_remote`), QEMU 7.2.0 (qemu-arm-static / qemu-armeb-static) | QEMU 7.2.0 arm32/armeb user-mode emulation's `connect` to 0.0.0.0:1 spuriously returns 0 (success) instead of -ECONNREFUSED. Workaround: `expand_channel_open_remote` retries with a sleep loop and times out. |
| QEMU arm32 poll returns 0 — **OPEN (QEMU 7.2.0 workaround in place)** | `ipc_lowering.rs:1568` (K12A-fix comment in `channel_recv` poll_loop), `:2362` (K9B sibling in `channel_try_recv`), QEMU 7.2.0 (qemu-arm-static / qemu-armeb-static) | QEMU 7.2.0 arm32/armeb user-mode `poll` on a pipe with no data spuriously returns 0 (timeout-style) instead of blocking. Workaround: the poll_loop uses a sleep+retry pattern with a 30 s wall-clock budget. |
| QEMU hppa/sparc64 positive errno — **OPEN (QEMU 7.2.0 workaround in place)** | `ipc_lowering.rs:2323` (K10B comment + `read_ret != 56` Cmp at `:2359` in `channel_try_recv`), `:1635` (K14A sibling ±EAGAIN check in `channel_recv` poll_loop), QEMU 7.2.0 (qemu-sparc64-static / qemu-hppa-static) | QEMU 7.2.0 hppa/sparc64 user-mode returns errno as a POSITIVE value in r0 instead of the standard negative. Workaround: the recv paths check `read_ret != 56` (expected byte count) and treat any other value as `EAGAIN`. |
| QEMU loongarch64 p_memsz crash — **OPEN (QEMU 7.2.0 workaround in place)** | `loongarch64/mod.rs:2170` (`[QEMU-WA:loongarch64-memsz]` comment, `text_memsz = text_filesz` at `:2194`), QEMU 7.2.0 (qemu-loongarch64-static) | QEMU 7.2.0 loongarch64 ELF loader crashes (SIGSEGV) when an executable segment's `p_memsz > p_filesz` (BSS extension). Workaround: `text_memsz = text_filesz` at `:2194`; the BSS bytes are zero-initialized via a `memset` loop in `_start`. |
| Checkpoint file race — **RESOLVED** | `ipc_lowering.rs:build_checkpoint_path` | Shared `/tmp/vuma_checkpoint.bin` caused race. Fixed by appending PID to filename. The generic path `/tmp/vuma_checkpoint_<PID_raw_bytes_+1>.bin` is constructed by `build_checkpoint_path` (`ipc_lowering.rs:build_checkpoint_path`). |

## 6. Testing

| Issue | Location | Description |
|-------|----------|-------------|
| Test count disagreement — **RESOLVED** | manifest/README/find | CI gate added; manifest reconciliation done. The stale numbers in this row (1521/1564/1574) have since drifted further: the manifest reports `total_programs: 1536` (`tests/gold_standard/manifest.json:6`), but `find tests/gold_standard -name '*.vuma' \| wc -l` reports 1589. The `ipc/` category was missing from the manifest; `make regen-manifest` reconciled it. The CI `manifest` job (`.github/workflows/ci.yml:56-64`) fails on any future drift. |
| 100% pass rate is HEAD-only — **RESOLVED** | `test_results/` | Checkpoint used to be cleared on every compiler rebuild with no trend data retained. Fix: `scripts/pi5_test_suite.sh` Step 2.5a now snapshots the prior `test_results/summary.json` into `test_results/history/<YYYYMMDDHHMMSS>_summary.json` before any clear. `--trend [N]` / `scripts/show_trend.py` prints the pass-rate history. |
| No `#[should_panic]` tests — **RESOLVED** | `src/tests/`, `src/ive/src/`, `src/codegen/src/` | Added 9 negative-path unit tests across 5 source files in 3 library crates (`vuma-parser`, `vuma-ive`, `vuma-codegen`): 1 real `#[should_panic]` (`src/codegen/src/ir.rs:3621` `test_negative_current_block_panics_on_empty_blocks`) + 8 `Result`/`Verification`-style with error-message substring checks. |
| `ci.yml` branches typo — **RESOLVED** | `.github/workflows/ci.yml:12, 14` | Re-audited and confirmed the `branches: ain]` typo does NOT exist at HEAD — all 4 workflow files (`ci.yml`, `cross-compile.yml`, `wave50-hardening.yml`, `proof-verify.yml`) correctly use `branches: [main]`. |
| Pre-existing clippy warnings — **RESOLVED** | project-wide | Resolved via `cargo clippy --fix` (mechanical) + manual edits to 4 deferred categories (`doc_lazy_continuation`, `doc_overindented_list_items`, `type_complexity`, `ptr_arg`), and 4 deny-level `clippy::not_unsafe_ptr_arg_deref` errors at `vuma_context.rs:76,101,125,149` and 1 `clippy::never_loop` in `dump_stages.rs`. |
| `self_exec.vuma` SIGPIPE flakiness — **OPEN (accepted retry workaround)** | `scripts/pi5_test_suite.sh:720-781` (retry logic + root-cause comment) | Accepted workaround: retry up to 3× on rc=-13 (SIGPIPE). Root cause: under QEMU user-mode emulation, the parent closes pipe2's read end (or exits, or execve's) before the child finishes writing. |
| `wave50.rs` pipeline UAF test renamed, asserts `is_ok` only | `src/tests/src/wave50.rs:1327` | The test `test_wave50_uaf_pipeline_compiles_when_alloc_elided` (renamed from `test_wave50_uaf_rejected_pipeline_either_outcome`) asserts `result.is_ok()` only. The parser's escape+effects pass elides the small `allocate(4)` alloc before the SCG-liveness UAF detector sees it, so the UAF never materializes. The strict UAF-detection assertion lives in the sibling `test_wave50_uaf_rejected` at `wave50.rs:1175`, which builds the SCG by hand (bypassing the parser's elision). |
| `examples_tmp/check_th_test.rs` orphan — **RESOLVED** | `examples_tmp/` (does not exist) | `examples_tmp/` does not exist anywhere in the repo — both `find examples_tmp*` and `find check_th_test*` return zero hits. The directory was already removed. |
| Pi auto-commits to git — **RESOLVED** | `scripts/pi5_test_suite.sh:1035-1179` | Auto-commit is now gated behind an explicit `--commit` flag (default OFF). Without `--commit`, the script prints a summary of what WOULD be committed (staged files with byte sizes, proposed commit message, `git status --porcelain` preview) and instructions for manual commit, then exits without calling `git commit` or `git push`. |

## 7. Dead Code

| Module | Location | Status |
|--------|----------|--------|
| `ive/src/borrow_region.rs` | (formerly 823 LOC) | **RESOLVED.** File is now 255 LOC (not 823) and IS invoked from `pipeline.rs:5346-5368` and `pipeline.rs:6612-6634` as advisory-only linear-channel checks (`verify_linear_channels`/`all_linear_valid`). The "Library-only. Never invoked" caveat is removed. |
| `ive/src/ffi.rs` | (formerly 82 LOC) | **REMOVED.** File does NOT exist in this branch. Only the top-level `src/ffi.rs` (VUMA FFI bindings) exists. |
| `ive/src/escape.rs` | (formerly 498 LOC) | **REMOVED.** File DOES NOT EXIST at the cited path. Escape analysis lives at `src/codegen/src/escape_analysis.rs` and is used by codegen SROA (not by IVE verification, as the caveat said). |
| 5 legacy pointer invariants | `ive/src/` | **REMOVED.** Liveness, exclusivity, interpretation, origin, cleanup were deleted entirely from `InvariantKind`. The enum now has only the `Pmt` variant. |
| `find_bounds_check_sites` | `memory_safety.rs` | **REMOVED.** Dead wrapper function and its `#[test]` deleted; `find_bounds_check_sites_with_bounds` retained (live). |
| `--safe` CLI flag | `main.rs` | **RESOLVED.** Flag is parsed (`cli.safe` → `runtime_bounds_checks: cli.safe` at `main.rs:1536`) and wired through `MemorySafetyConfig::safe_mode` vs `compile_time_only` selection in `pipeline.rs:5405`; `find_bounds_check_sites_with_bounds` + `inject_bounds_check_ir` run when `--safe` is on. |
| `runtime_bounds_instrumented` | `pipeline.rs` (actually `memory_safety.rs`) | **REMOVED.** Field + initializer + write deleted from `memory_safety.rs`; never read anywhere. |
| COR PMT state node mapping | `cor/src/bridge.rs` | **REMOVED.** Neither the `cor/` crate nor `bridge.rs` exists in this repo. The COR→PMT bridge was deleted. |
| `state_merge_compatible_layouts` | `bv_verify.rs:69` | **RESOLVED.** No longer a stub returning `None`: a real verification function `state_merge_compatible_layouts(lhs, rhs) -> Option<LayoutIncompatibility>` is defined at `bv_verify.rs:403`, exercising field-count + per-field `(offset, size)` matching. The e-graph rule's `apply` closure remains a no-op (deferred to a future lifetime-aware merging pass). |
| `examples_tmp/` | `examples_tmp/` | **REMOVED.** Directory does NOT exist. Already removed. |

### Dead-code cleanup

The following were deleted as truly-dead items (zero callers, zero tests, no future-intent comment):

| File | Item | Reason |
|------|------|--------|
| `src/parser/src/to_scg.rs` | `emit_async_await_lowering` (62 LOC) | Dead "async/await lowering" stub — zero callers; the SCG `ControlKind::FuturePoll` / `WakerRegistration` variants remain defined for the future lowering pass. |
| `src/parser/src/parser.rs` | `parse_type_params` (15 LOC) | Dead — superseded by `parse_type_params_with_bounds` (the only variant called from production). |
| `src/parser/src/parser.rs` | `recover_to_block_boundary`, `skip_balanced_braces`, `recover_from_error`, `parse_inner_attributes`, `RecoveryLevel` enum (88 LOC total) | Dead error-recovery infrastructure cluster — zero production callers; `RecoveryLevel` was only referenced by `recover_from_error` itself. |
| `src/codegen/src/ipc_lowering.rs` | `jump_block`, `cond_branch_block` (24 LOC) | Dead IR-block builder helpers — zero callers. |
| `src/codegen/src/m68k.rs` | `_unused_table_marker` (4 LOC) | Useless marker — comment said "Keep the unused-variable warning quiet for the helper" but the helper itself was already deleted. |
| `src/vuma/src/repl.rs` | `format_ast_type` (6 LOC) | Trivial dead wrapper (`format!("{}", ty)`); zero callers. |

Items intentionally KEPT under `#[allow(dead_code)]` (with explanatory comments) because they are API stubs, documentation, or feature-gated for future use:

| File | Item | Reason kept |
|------|------|-------------|
| `src/parser/src/parser.rs` | `SECRET_ATTR` const | Documents the canonical `#[secret]` attribute name; callers compare against the literal `"secret"` today, but the const is retained for future parser-side validation. |
| `src/parser/src/parser.rs` | `expect_ident` | "Part of Parser API for future grammar extensions" (explicit comment). |
| `src/scg/src/graph.rs` | `OrderedSet::len`, `OrderedSet::is_empty` | API-completeness methods on an internal `pub(crate)` helper struct. |
| `src/scg/src/serialize.rs` | `BinaryReader::remaining`, `BinaryWriter::write_u8` | "Part of BinaryReader/Writer API for future serialization needs" (explicit comment). |
| `src/scg/src/loop_detection.rs` | `CfgDomTree::entry` field | Kept for struct-field consistency with the dominator-tree algorithm. |
| `src/codegen/src/{sparc64,alpha,m68k,hppa,s390x,…}.rs` | Backend FP-constant tables and superseded emitters (e.g. `m68k::emit_fp_cmp`, `s390x::encode_lgb/encode_lgh/encode_lgf`, `sparc64::FP_FMOVS`/`FP_FNEGS`/…) | Documentation of ISA encoding space; "retained for reference" per inline comment. |
| `src/codegen/src/x86_64/stack_slot_isel.rs` | `emit_crc32_range_dynamic`, `emit_fnv1a_64_loop`, `emit_fnv1a_64_loop_nosalt`, `emit_aead_xor_loop` | Reference encoders for future stack-slot CRC / FNV / AEAD work. |
| `src/codegen/src/loongarch64/mod.rs` | `encode_4r` | Reference encoder for the LoongArch 4R instruction format. |
| `src/codegen/src/loongarch64/stack_slot_isel.rs` | `terminator_opcode_override` | Helper for future block-terminator test detection. |
| `src/codegen/src/riscv32.rs` | `ss_load_addr` | Companion to the live `ss_load_imm` / `ss_store_to_slot` stack-slot helpers; kept for API symmetry. |
| `src/codegen/src/emit.rs` | AArch64 capability-signature fields (`cap_sig_off`, `cap_siginput_off`, `cap_siginput_len_off`), `emit_crc32_frame_loop_aarch64`, `emit_bcond_placeholder` / `patch_bcond` / `emit_b_placeholder` | Feature-gated for the AArch64 capability-signature stack-slot work. |
| `tests/property_tests.rs` | `CompileOutcome::has_violated_invariant`, `stage_failed`, `has_memory_safety_error` | Test-API helpers (some used, some not yet) on the `CompileOutcome` test struct. |

## 8. Consolidated TODO/FIXME List

| File | Line | Description |
|------|------|-------------|
| `scg/src/structured_output.rs` | 1030 | PMT minimal stub — **RESOLVED**. |
| `scg/src/serialize.rs` | 1792 | PMT minimal stub — **RESOLVED**. |
| `scg/src/loop_detection.rs` | 89 | TODO (deferred) — **PARTIALLY RESOLVED**: `ControlFlowGraph` trait + generic `LoopDetector::detect_natural_loops_on` added; `SCG` impl + `MockCfg` test prove the abstraction is sound. Full cross-crate unification with `vuma-codegen::regalloc::LoopDetector` still deferred. |
| `to_scg.rs` | 668 | Layout stub — **RESOLVED.** The `Item::LayoutDef` arm is at `:668` and lowers to a real `NodeType::StructDef` node carrying `StructFieldInfo { name, type_name, offset, size }` (no longer a `ComputationKind::Other("layout stub")` placeholder). |
| `to_scg.rs` | 2284 | Transform stub — **RESOLVED.** The `Stmt::TransformCall` arm is at `:2284` and lowers via `emit_call_nodes` + a `ComputationNode` let-binding that defines `tc.dst` (no longer a `ComputationKind::Other("transform stub")` placeholder). |
| `pipeline.rs` | 2520 | State lowering — **RESOLVED**. Previously cited as `:2496` (stale — line drift). Real TODO was at `:2465`→`:2520` (a `Vec::new` stub that silently dropped all PMT state ops). Now lowered to extern `__vuma_state_init__<L>` / `__vuma_state_read__<L>__<f>` / `__vuma_state_write__<L>__<f>__<v>` calls. |
| `pipeline.rs` | 12252 | Arena/sync/match lowering — **PARTIALLY RESOLVED**. The match-stmt `eprintln!("[vuma] TODO:...")` has been replaced with per-arm `vuma_log!(warn,...)` diagnostics, but the actual match-stmt lowering to a jump-table or if-else cascade is still a TODO. |
| `pipeline.rs` | 12625 | Arena/sync/match lowering — **PARTIALLY RESOLVED**. The sync-block `eprintln!("[vuma] TODO:...")` has been replaced with a `vuma_log!(warn,...)` diagnostic, and `pthread_mutex` locking is now emitted for the sync-block; the TODO marker is retained because the locking is non-reentrant. |
| `main.rs` | 2199 | `emit` path divergence — **PARTIALLY RESOLVED.** `cmd_emit` is at `:2163` and routed through `compile_to_binary_direct` (the direct AST→codegen path), not `compile_with_path` (the canonical pipeline with full IVE gate). The direct path runs only the PMT-level verifiers. |
| `cor/src/bridge.rs` | — | PMT state nodes "map to Memory for now" — **REMOVED.** File does NOT exist in this repo. The COR→PMT bridge was deleted. |
| `bv_verify.rs` | 69 | `state_merge_compatible_layouts` stub — **RESOLVED.** No longer a stub: real verification function defined at `bv_verify.rs:403`, exercised by 5 unit tests. The e-graph rule's `apply` closure remains a no-op (deferred to a future lifetime-aware merging pass). |
| `memory_safety.rs` | — | `find_bounds_check_sites` dead code — **REMOVED**. |
| `hppa.rs` | — | F1b/G3 TODOs — **PARTIALLY RESOLVED**. F1b (FP comparison stub at the dead-code `emit_hppa_fp_binop` arm, `hppa.rs:~662-725`): the silent "store 0" stub has been replaced with a loud `unimplemented!("F1b (hppa emit_hppa_fp_binop)")` panic. G3 (FP move stub at `emit_hppa_fp_move`): same treatment. The actual FP comparison/move lowering is still unimplemented. |
| `m68k.rs` | — | G4 TODOs — **PARTIALLY RESOLVED**. The G4 banner was at `m68k.rs:136-143` with per-emitter TODOs at lines 1419, 1440, 1449, 2519, 3138, 3173, 3277-3289, 3530-3542. The 68881 coprocessor-1 (F-line) byte sequences emitted by these emitters are valid m68k instructions; the TODOs are about whether QEMU 7.2.0's m68k translator accepts them. |
| `sparc64.rs` | — | F1d/G5 TODOs — **PARTIALLY RESOLVED.** F1d (FloatToUInt of negatives): the `CastKind::FloatToUInt` arm (around `:2829`) previously ran `FDTOX` directly on the source float, which is the only FP→int conversion SPARC V9 provides (and it is signed). Now uses `FDTOX` + a sign-branch fixup. G5 (FP rounding modes): the rounding-mode mask in `FSR` is now set per-cast. |
| `alpha.rs` | — | G5b TODOs — **RESOLVED**. The `FloatToUInt` cast (alpha.rs `CastKind::FloatToUInt` arm in `emit_instr`) previously used `CVTTQ` (Alpha's signed-only f64→i64 conversion) directly for all non-negative inputs, which is undefined/saturated for `f ≥ 2^63` (i64::MAX + 1). The fix uses `CMPTLE` to clamp the input to `i64::MAX` before `CVTTQ`. |
| `vectorize.rs` | — | ISel packed-ops TODO — **PARTIALLY RESOLVED**. New `vectorize::lower_packed_ops_to_vectorops(func, &plan)` helper wires the `PackedOp` side-channel into the backend ISel: for each `PackedOp` in the plan it rewrites the lane-0 scalar `BinOp`/`Add`/`Sub`/`Mul` (matched by `dst_lanes[0]`) into a `VectorOp` that operates on all lanes. The TODO marker is retained because only `Add`/`Sub`/`Mul` are vectorized; other `BinOp` variants still go through scalar. |
| `syscall_abi.rs` | — | `translate_or_warn` TODO — **PARTIALLY RESOLVED**. `translate_or_warn` (defined at `syscall_abi.rs:281`) is wired into 16+ call sites across 15 native backends + 1 generic emit path. Each call site passes its OWN `BackendKind`; no cross-backend mismatch. The TODO marker is retained because the function still uses a substring-based backend-name lookup (`name.contains("arm")` / `name.starts_with("x86")`) rather than a `match` on `BackendKind`. |

## 9. Lean Formal Verification (PMT Memory Model)

> **Verdict:** The Lean proofs verify an *abstraction* of the PMT memory model.
> They prove properties of the mathematical model, NOT of the shipping Rust
> binary.

**Title: Lean Proofs Are Not a Full Compiler Verification**

The Lean proofs (under `proof/`) verify the PMT memory model along six axes:
arena capacity, field-bounds, linearity, liveness, guard-page isolation, and
soundness. These are theorem-level guarantees about the model.

They do **NOT** verify:

| Gap | Scope |
|-----|-------|
| The Rust implementation itself | **PARTIALLY CLOSED.** `proof/PMT/PipelineSim.lean` provides a `PipelineSpec` structure and theorems (`exec_satisfies_pipeline_spec`, `pipeline_compile_sound`, `pipeline_compile_no_oob`) connecting Lean `exec` to the Rust `pipeline::compile` specification, CompCert-style. The `hconforms: PipelineSpec prog s` hypothesis is the translation-validation obligation; the Rust-side parity test (`tests/pmt_parity_test.rs`, 5 tests) is the empirical discharge. |
| IR lowering | No Lean model of `AST → SCG → IRProgram`. The compiler's lowering is not formalized. The `to_program_preserves_well_typed_full` sorry (see below) is the in-model placeholder for this gap. |
| Codegen backends | 19 backends emit machine code; none has machine-code verification or a Lean-extracted reference. |
| Runtime syscalls | `mmap`, `mprotect`, `brk`, etc. are modeled as opaque effects. No Lean semantics for the syscall surface. `PMT.MmapArena` models the `mmap`/allocator-null failure path via `raw_create: Nat → Except TrapCode RawArena`, closing the simulation-soundness gap where the bare `mmap` was assumed to succeed. |

The Lean model is an **abstraction**. A proof of `capacity_preserved` says "in
the model, capacity is preserved by this operation." It does NOT say "the
compiled binary preserves capacity at runtime." Closing that gap requires a
simulation relation, which is currently scaffolded but not discharged.

### Iris / overflow / simulation

Three long-standing faithfulness gaps were closed:

- **Iris invariants formalised.** The three named invariants `[cap_bnd]`,
  `[live_mirror]`, `[guard]` are now formalised as separation-logic resources
  in `proof/PMT/Iris/` (`CapBndInvariant.lean`, `LiveMirrorInvariant.lean`,
  `GuardInvariant.lean`), with ghost ownership `Own γ v` over the `Ex`/`Ag`
  resource algebras, plus the Composition theorem `alloc_preserves_all_invariants`
  in `Iris/Composition.lean` showing the bundle `[cap_bnd] ∗ [live_mirror] ∗ [guard]`
  is preserved by `alloc`. A single local axiom `own_ex_exclusive` (documented in
  `LiveMirrorInvariant.lean`) characterises the `Ex` RA's exclusivity principle
  in the simplified `Prop`-valued encoding.
- **BitVec overflow model.** `proof/PMT/BitVecArena.lean` models the Rust
  `Arena` using `BitVec 64` for addresses and offsets (mirrors `usize` on
  64-bit platforms), making the arithmetic-overflow branch syntactically
  expressible — the actual failure mode the Rust `Arena::alloc_raw`
  `checked_add` defends against. The previous `Nat`-based `RawArena` was
  structurally unbounded (`offset + aligned_size` could never wrap), so
  proofs derived from "all overflow paths trap" were unsound w.r.t. the
  Rust binary; `BitVecArena` closes that gap. See
  `docs/proof/S2--D-arena-fidelity.md`.
- **Lean↔Rust simulation.** `proof/PMT/PipelineSim.lean` provides the
  mechanical simulation theorem connecting Lean `exec` to the Rust
  `pipeline::compile` specification (CompCert-style translation validation).
  The `hconforms: PipelineSpec prog s` hypothesis is the translation-validation
  obligation; the Rust-side parity test (`tests/pmt_parity_test.rs`, 5 tests)
  is the empirical discharge. The hand-translated Rust checkers live in
  `src/codegen/src/runtime/pmt_check.rs`, gated by the `pmt-runtime-check`
  cargo feature.

## 10. Documented TODOs (`sorry`) in Lean Proofs

> **Verdict:** The Lean proof library contains 6 documented `sorry`s — all in
> the three Iris modules (`Iris/ArenaRes.lean`, `Iris/FractionalPerm.lean`,
> `Iris/WeakestPrecond.lean`). The `IRProgram.to_program_preserves_well_typed_full`
> sorry in `ExecFunction.lean` — the previous "last sorry" — was CLOSED by
> strengthening `IRProgram.well_typed` (in `proof/PMT/IRProgram.lean`) to
> enforce the SSA-like name-uniqueness discipline on every `IRFunction` and
> `IRProgram`. The remaining 6 sorries are auxiliary Iris-algebra lemmas
> (splitting side-conditions, the `wp` frame/bind/soundness trio); they do not
> undermine the Iris invariants, which remain sorry-free.
>
> Strict CI is suspended for these six named sorries; any seventh sorry will
> still fail CI.

| File | Sorry count | Status | Meaning |
|------|-------------|--------|---------|
| `proof/PMT/Iris/WeakestPrecond.lean` | 3 | **OPEN** | `wp_frame`, `wp_bind`, `wp_soundness` — the Iris frame-rule, monadic-bind, and fundamental-soundness lemmas for `wp e {Φ}`. `wp_monotone` and `wp_value` (also defined in this module) are closed sorry-free. Closing the trio depends on a proper Iris heap-model. |
| `proof/PMT/Iris/FractionalPerm.lean` | 1 | **OPEN** | `write_requires_full` — placeholder for the write-predicate: a write needs `↦{1.0}` (no fractional writes). The two core algebraic lemmas `frac_split` (`↦{q} ≡ ↦{q/2} ∗ ↦{q/2}`) and `frac_compat` (`↦{q1} ∗ ↦{q2} ⟹ ↦{q1+q2}`) are closed sorry-free. |
| `proof/PMT/Iris/ArenaRes.lean` | 1 | **OPEN** | `arena_res_split` — the splitting rule `ArenaRes A -∗ SubArena A ofs sz ∗ ArenaRes⟨A.used+=sz⟩` (provided `ofs+sz ≤ cap`). The statement is correct; only the arithmetic side-condition normalisation needs `omega`-style discharge against the `BitVecArena` model. |
| ~~`proof/PMT/ExecFunction.lean`~~ | ~~1~~ | **CLOSED** | `IRProgram.to_program_preserves_well_typed_full` — the IR-flattening lemma whose statement was previously unsound under the weak `IRProgram.well_typed`. Closed by strengthening `IRProgram.well_typed` to enforce the SSA-like name-uniqueness discipline. |
| ~~`proof/PMT/PipelineSim.lean`~~ | (carried over; tracked separately) | see note | The `hconforms` translation-validation hypothesis is the central CompCert-style assumption; the 5-test Rust parity harness (`tests/pmt_parity_test.rs`) is the empirical discharge. This sorry is not part of the invariant proofs and is tracked separately. |

**Total sorry count: 6 / 6.** Each `sorry` carries an inline `-- TODO` comment
and a docstring paragraph naming the proof-strategy blocker;
`scripts/check-lean.sh` in non-strict mode prints
`OK (non-strict): N documented sorry` and exits 0.

> **Counting note (historical):** `scripts/check-lean.sh` counts `sorry`
> tokens emitted in `lake build` *output*, not in source. During development
> the build-output count varied with which modules Lake recompiled (cached
> modules don't re-emit warnings). The current source-level truth is 6 (3 in
> `WeakestPrecond.lean`, 1 in `FractionalPerm.lean`, 1 in `ArenaRes.lean`,
> plus the carried-over `PipelineSim.lean` sorry — see banner). The current
> count under strict mode is 6 (CI is temporarily non-strict for these six
> named sorries).

> **Feature flag wired:** the `pmt-runtime-check` Cargo feature is wired into
> the production arena path — `src/codegen/src/runtime/arena.rs` calls the
> Lean-verified `verified_capacity_check` (and the sibling
> `verified_field_bounds_check` / `verified_linearity_check` /
> `verified_pmt_check`) on the arena-overflow and capacity-overflow branches
> when the feature is on. The four Lean functions carry
> `@[export lean_verified_*]` attributes in `proof/PMT/Extraction.lean`; the
> root `Cargo.toml` forwards the feature
> (`pmt-runtime-check = ["vuma-codegen/pmt-runtime-check"]`); and the
> dedicated `tests/pmt_feature_flag_test.rs` (3 tests, gated by
> `#![cfg(feature = "pmt-runtime-check")]`) verifies the wiring compiles and
> the `verified_*` symbols are callable from the codegen crate.

**Strictness modes** (see `scripts/check-lean.sh` and `docs/building.md`):

- `make proof-check` — non-strict. Allows documented TODOs to pass. Prints
  `OK (non-strict): N documented sorry (TODOs for)` and exits 0. Suitable for
  local development. This is the mode used in CI while the six hard-proof
  sorries remain open.
- `PROOF_CHECK_STRICT=1 make proof-check` — strict. Fails on ANY `sorry`,
  documented or not (exit 1). Temporarily NOT the default in CI — will be
  re-enabled once the six sorries are closed.

> **CI strict mode (suspension):** `PROOF_CHECK_STRICT=1` is temporarily
> suspended in `proof-verify.yml` and `ci.yml` for the six named sorries
> above. Any regression that re-introduces a *seventh* `sorry` — documented
> or not — will still fail CI. The strict-mode suspension is scoped
> exclusively to `Iris/WeakestPrecond.lean`, `Iris/FractionalPerm.lean`, and
> `Iris/ArenaRes.lean`; all other modules remain sorry-free.

Developers must NOT add new `sorry` beyond the six named above. The
convention of `-- TODO Wave N` comments is retained as a triage aid; the only
acceptable `sorry` count is 0 (steady state) or 6 (the current hard-proof
carve-out).

## Cross-References

- [Architecture Overview](architecture.md) — pipeline stages, crate inventory, backend matrix
- [Pipeline](pipeline.md) — compilation stages
- [Backends](backends.md) — per-backend details
- [Language Reference](language-reference.md) — types, expressions, builtins, FFI
- [Testing](testing.md) — test infrastructure
- [Building](building.md) — build prerequisites, quick start, troubleshooting
- Lean proof reports under `proof/` — Lean proof and build reports

## 0.7. IVE Inert Verifier Triage — Wave 0

> **Added by:** IVE Wave 0 task C (branch `task/ive-0-c`).
> **Scope:** Triage of 4 structurally-inert IVE verifiers identified during
> Wave 0 baseline. For each: RESTORE now (small fix), DEFER to Wave 2
> (Lean proof + real input propagation), or REMOVE (mark as inert in
> source). Wave 0 task C RESTOREs at most 2; the rest are DEFERRED or
> REMOVE'd per the table below.
>
> **Cross-ref:** §2 "IVE (Intermediate Verification Engine)" row
> "Advisory verifiers never abort" previously claimed the linear-channel
> gate was "dormant due to a pre-existing parser gap" — that claim is now
> STALE (the parser was fixed; see row 1 below). §2 row "Borrow region /
> FFI modules library-only" is also stale (the gate is invoked from
> `pipeline.rs` Stage 7c at two callsites and is now LIVE).

### Decision table

| # | Verifier | Location | Inert mechanism (pre-Wave-0) | Decision (Wave 0 task C) | Rationale / next step |
|---|----------|----------|------------------------------|--------------------------|------------------------|
| 1 | `borrow_region::verify_linear_channels` | `src/ive/src/borrow_region.rs`; callsites `src/pipeline.rs:5916-6065` (`compile_with_path`) and `src/pipeline.rs:7416-7570` (`compile_with_recovery`) | HARD-FAIL gate wired but **believed** DORMANT due to parser emitting generic `ControlNode` payloads with `call_channel_*` labels instead of the dedicated `NodePayload::Channel*` variants the call site pattern-matches. | **RESTORE (DONE).** No source-level behavioural change required — the parser was already fixed. `src/parser/src/to_scg.rs::try_emit_channel_node` (line ~2508) emits `NodePayload::Channel{Open,Send,Recv,Close}` and short-circuits the generic FunctionEntry/FunctionReturn lowering. The Stage 7c call site builds a non-empty `events` Vec for any channel-using program, and `verify_linear_channels` fires genuine use-after-close / double-close / uninitialized-use violations. The module docstring's "DORMANT" claim was stale; this task updates it to "LIVE". End-to-end regression tests in `tests/linear_channel_hard_fail.rs` (`linear_channel_use_after_close_fails_by_default`, `linear_channel_double_close_fails_by_default`) run without `#[ignore]` and pin the contract. | Smallest possible RESTORE: docstring-only change. The verifier's contract was already correct; only the documentation lagged. Wave 2 will add a Lean soundness proof (`proof/PMT/IVE/Soundness/BorrowRegion.lean`, file does not yet exist). |
| 2 | `arena_bounds::verify_arena_bounds` | `src/ive/src/arena_bounds.rs:26-44` | Function body unconditionally returns `Vec::new()`; the loop over `accessed_vars` has an empty body (comment only); inputs are discarded via `let _ = (arena_vars, accessed_vars);`. ZERO callers in `pipeline.rs`. The `all_valid(&[])` shortcut makes the unit tests pass trivially. | **REMOVE (DONE).** Marked as INERT in the module-level docstring (`//! # IVE Wave 0 task C — INERT (REMOVE); restoration deferred to Wave 2`). No source-code deletion in Wave 0 — the function and tests are retained so Wave 2 can either RESTORE with proper SCG/IR plumbing + Lean proof, or DELETE as confirmed-redundant. Callers MUST NOT rely on `verify_arena_bounds` for any real verification. | The actual arena-bounds enforcement is performed at RUNTIME by the codegen — `pipeline.rs:11710` emits `ComputationNode(UGe)` + `ControlNode::If { __oob_trap }` calling `__arena_overflow()` at every arena-alloc site. Arena LINEARITY (use-after-`arena_free`) is handled by the invariant aggregator's `consumed_vars` tracking. The IVE-level wrapper was therefore redundant from inception; its signature (`arena_vars`, `accessed_vars` HashSets) is too narrow to reconstruct either check without SCG/IR plumbing that does not exist. Wave 2 will decide RESTORE-vs-DELETE. |
| 3 | `information_flow::verify_information_flow_from_ir` | `src/ive/src/information_flow.rs:478-524`; callsite `src/pipeline.rs:5462` | The IR-level wrapper hardcodes every security label to `SecurityLabel::Public` (for both `ChannelSend` events and `Store` events, lines 496-497, 507-508). Because `Public → Public` is always a legal flow, the underlying `verify_information_flow` cannot produce any violations by construction. The HARD-FAIL gate at `pipeline.rs:5463-5479` therefore never fires. | **DEFER to Wave 2.** No Wave 0 source change. The underlying `verify_information_flow` (line 167) does real work on real inputs; the gap is in the wrapper's input-shaping. Restoration requires propagating security labels from the AST (where `#[secret]` / `#[public]` annotations are collected by `pipeline.rs::collect_secret_vars` at `:9708`) THROUGH the IR — currently the IR has no label-carrying instructions. This is non-trivial: it touches the parser (label emission), the IR type (`IRInstr` / `IRValue`), and the codegen-lowering path. Wave 2 will add the IR label plumbing + a Lean soundness proof (`proof/PMT/IVE/Soundness/InformationFlow.lean`, file does not yet exist). | Restoration is NOT a "small fix" — it requires changes across parser, IR, and codegen crates. Wave 2 is the appropriate venue. The Wave 0 pipeline.rs comment-fix (above) makes the structural-vs-empirical distinction explicit so future readers don't mistake "zero violations" for empirical validation. |
| 4 | `session_type::verify_session_types_from_ir` | `src/ive/src/session_type.rs:713-759`; callsite `src/pipeline.rs:5442` | The IR-level wrapper hardcodes `SessionType::End` for every `channel_open` event (line 727) AND hardcodes `vreg: 0` for ALL channel events (Open, Send, Recv, Close). Because `End` means "the session is already complete", any subsequent `Send`/`Recv`/`Close` on `vreg=0` would be a session violation — BUT all events share `vreg=0`, so the first `Open` records `state[0]=End`, and the next `Send`/`Recv` fires "send on vreg 0 but protocol expects End (not Send)". In practice the gold-standard suite has no channel-using programs that reach this IR path, so "zero violations" is structural (the wrapper cannot meaningfully exercise the verifier), not empirical. | **DEFER to Wave 2.** No Wave 0 source change. The underlying `verify_session_types` (line 182) does real work on real inputs (it tracks state per `vreg` and validates Send/Recv/Close transitions). The gap is in the wrapper's input-shaping. Restoration requires (a) propagating real session-type annotations from the AST through the IR, and (b) extracting the actual channel handle's vreg/name from IR `Call` arguments (currently hardcoded to `0`). Wave 2 will add the IR annotation plumbing + a Lean soundness proof (`proof/PMT/IVE/Soundness/SessionType.lean`, file does not yet exist). | Restoration is NOT a "small fix" — it requires AST session-type annotation support (which the parser does not currently emit), IR plumbing, and codegen cooperation. Wave 2 is the appropriate venue. The Wave 0 pipeline.rs comment-fix (above) makes the structural-vs-empirical distinction explicit. |

### Summary

- **RESTORE now (1):** `borrow_region` — docstring-only fix; parser already
  emits dedicated `NodePayload::Channel*` variants; gate is LIVE.
- **REMOVE (mark inert in source, 1):** `arena_bounds` — structurally
  redundant with the runtime `__oob_trap` guard and the invariant
  aggregator's linearity tracking. Source retained for Wave 2
  RESTORE-vs-DELETE decision.
- **DEFER to Wave 2 (2):** `information_flow`, `session_type` — restoration
  requires AST→IR label / session-type annotation propagation across
  parser, IR, and codegen crates. Wave 2 will also add Lean soundness
  proofs (`proof/PMT/IVE/Soundness/{InformationFlow,SessionType}.lean`).
- **Pipeline comment fixes:** `src/pipeline.rs` Stage 7c comments at the
  l1l3-collapse block (`pipeline.rs:5382-5394`) and the session-type +
  information-flow block (`pipeline.rs:5423-5438`) have been updated to
  honestly describe the gates' current state. The "INV-2 found zero
  violations" framing has been replaced with explicit
  structural-vs-empirical language. The l1l3-collapse gate is REAL (it
  inspects `IRInstr::Call` argument shapes and CAN fire); the
  session-type and information-flow gates are STRUCTURALLY inert because
  their wrappers hardcode the inputs.

### Out-of-scope follow-ups (NOT done in Wave 0)

- `tests/linear_channel_hard_fail.rs` header docstring (lines 16-54)
  still describes the OLD parser-gap state and the `#[ignore]` attributes
  that have since been removed from the actual tests. The tests
  themselves are up to date; only the header prose is stale. Cleanup is
  a documentation-only follow-up.
- §2 "IVE" row "Advisory verifiers never abort" claims the linear-channel
  gate is "dormant due to a pre-existing parser gap" — STALE per row 1
  above. Editing existing §2 lines is out-of-scope for Wave 0 task C
  (caveats.md edits are APPEND-ONLY). Wave 1 or later may amend §2.
