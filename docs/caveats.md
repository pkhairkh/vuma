# Caveats and Known Issues

> Current caveats and known limitations in the VUMA compiler. Each entry
> is keyed to the file (and where useful, the symbol) where the
> limitation lives so developers can find and fix it.

**How to use this file.** Every caveat here is a *current, real*
limitation. Resolved issues, removed stubs, and historical audit
reports have been pruned — only live caveats remain. When a citation
references a symbol rather than a line, grep for the symbol
(e.g. `rg -n 'TargetAgnosticRegAlloc' src/codegen/src/regalloc.rs`)
rather than trusting any line number verbatim, since lines drift as
code is edited above.

---

## 1. Build-time dependencies

### 1.1 Z3 is a hard dependency (`libz3-dev`)

| Aspect | Detail |
|--------|--------|
| Crate | `src/ive/Cargo.toml` (`z3 = "0.20"`) |
| Role | Z3 discharges the IVE verification conditions (contract / invariant / linearity / information-flow). Without Z3, the compiler cannot produce a verified binary. |
| Host install | Debian/Ubuntu: `apt install libz3-dev`; macOS: `brew install z3`; Arch: `pacman -S z3` |
| Failure mode | `cargo build` fails at link time with `could not find z3` / `-lz3 not found` if the system library is not present. |

The `vuma-ive` crate statically links against the system `libz3`.
There is no feature flag to disable it — Z3 *is* the verifier now
(it replaced the old Lean FFI bridge that was deleted).

### 1.2 Rust nightly-2026-03-01

The `rust-toolchain.toml` pins `nightly-2026-03-01` with the
`rustfmt`, `clippy`, `rust-src` components and the
`aarch64-unknown-linux-gnu` + `aarch64-unknown-none` targets. The
bare-metal kernel crate and `naked_asm`/inline-asm usages require
nightly; building on stable is unsupported.

---

## 2. Code generation

### 2.1 Stack-slot ISel is the only production code-emission path

Per the allocator classification audit
([`scripts/audit/allocator_classification.md`](../scripts/audit/allocator_classification.md),
commit `83846368`), the 19 backends split 6 / 12 / 1 by what
`allocate_registers` actually invokes:

| Backend(s) | Allocator wired in `allocate_registers` | Encoded bytes come from |
|------------|------------------------------------------|-------------------------|
| `aarch64` (direct), `aarch64_be` (inherits via wrapper delegation) | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (`backend.rs:3099`, `TargetAgnosticRegAlloc::new` at `:3114`) | Stack-slot ISel baseline (`emitter.emit_function(func, None)`, `backend.rs:3180`) |
| `x86_64` (direct) | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (`x86_64/mod.rs:4081`) | Stack-slot ISel baseline (`stack_slot_isel::allocate_registers`, `x86_64/mod.rs:4143`) |
| `riscv64` (direct) | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (`riscv64.rs:6542`) | Stack-slot ISel baseline |
| `ppc64` (direct), `ppc64le` (inherits via wrapper delegation) | Real — `TargetAgnosticRegAlloc` via `try_real_regalloc` (`ppc64/mod.rs:3011`) | Stack-slot ISel baseline |
| `arm32`, `armeb`, `mips64`, `mips64be`, `riscv32`, `x86_32`, `loongarch64`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa` | Stack-slot ISel (pure; or `use_real_regalloc=false` default whose `_real` greedy-stub branch is taken only inside `#[cfg(test)]` modules) | Stack-slot ISel |
| `wasm32` | Wasm-structured — no registers; vregs → Wasm locals; IR lowered directly to Wasm bytecode via `lower_function` (`wasm32/mod.rs:4631`) | Wasm bytecode |

**Counts.** 6 backends wire a real `TargetAgnosticRegAlloc` (4 direct —
`aarch64`, `x86_64`, `riscv64`, `ppc64` — plus 2 inherited via wrapper
delegation — `aarch64_be` → aarch64, `ppc64le` → ppc64). 12 backends are
pure stack-slot ISel (`arm32`, `armeb`, `mips64`, `mips64be`, `riscv32`,
`x86_32`, `loongarch64`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`).
1 backend (`wasm32`) is Wasm-structured and has no registers to allocate.
Total: 19. (The pre-audit caveat text counted "15 of 19" by mis-bucketing
`aarch64_be`, `ppc64le`, and `wasm32` into the stack-slot column.)

**Metadata-only caveat (critical).** Even on the 6 backends with a real
allocator wired, the real allocator runs **only as an annotation pass**:
it computes a `RegAllocResult` which
`regalloc_emit::annotate_with_regalloc` (`regalloc_emit.rs:82-92`) uses
to overwrite each `AllocatedInstruction`'s `reads` / `writes`
physical-register metadata. **The `encoded` byte stream is NOT modified**
— it always comes from the stack-slot ISel baseline invoked *before*
`try_real_regalloc`. The `reads` / `writes` metadata is consumed by
disassemblers, debuggers, and downstream tooling; it does not affect
emitted code. As a result **no VUMA backend emits register-based code in
production today**: all 18 native backends emit stack-slot-spill bytes
and `wasm32` emits Wasm bytecode. The `emit_function_regalloc` plumbing
that would re-emit bytes from a `RegAllocResult` exists at `emit.rs:1056`
but is unused in production — it is reachable only via
`emitter.emit_function(func, Some(alloc))`, which no production
`allocate_registers` calls. `LinearScanAllocator` (`regalloc.rs:1208`,
the older AArch64-specific linear-scan allocator with hardcoded
caller/callee-saved GPR+SIMD lists) is **test-only**:
`LinearScanAllocator::new` is invoked only inside `#[cfg(test)]` modules
(`regalloc.rs:4738+`, `emit.rs:9188+`).

**Implication.** On the 12 pure stack-slot backends every arithmetic /
load / store operation performs two extra memory accesses (load operands
→ operate → store result), so runtime performance is bounded by the
spill path even when physical registers are free. Generated code is
correct (verified by the 12-backend stack-slot correctness sweep,
468/468 PASS — see
[`scripts/audit/wave2_stackslot_results.md`](../scripts/audit/wave2_stackslot_results.md))
but is not benchmark-grade. **There is currently no performance gap
between the 6 "real" and 12 stack-slot backends**, because the "real"
path is metadata-only: every backend emits stack-slot-spill code at
runtime. A `~2–5×` speedup relative to today remains *theoretical* and
would require a future wave to make `emit_function_regalloc` consume
the `RegAllocResult` and re-emit register-based bytes (the plumbing
exists; the call site does not). The previous caveat wording ("~2–5×
slower than the linear-scan backends") was misleading and has been
removed.

**Why it's still in place.** The `TargetAgnosticRegAlloc` is
`TargetDesc`-driven, and wiring each remaining backend up requires
populating a complete, validated `TargetDesc` (register classes,
caller/callee-saved sets, ABI register roles, frame layout). Until
that work is finished for a given backend, the stack-slot path is the
safe fallback. See `src/codegen/src/target_desc.rs` and
`src/codegen/src/regalloc.rs` (`TargetAgnosticRegAlloc`). The full
per-backend classification with file:line citations for all 19 backends
lives in
[`scripts/audit/allocator_classification.md`](../scripts/audit/allocator_classification.md);
[`docs/backends.md`](backends.md) §1 carries the matching `Regalloc`
column.

### 2.2 wasm32 fork emulation is non-isolating

| Aspect | Detail |
|--------|--------|
| File | `src/codegen/src/wasm32/mod.rs` (file-level doc); `src/codegen/src/ipc_lowering.rs` (`wasm32_fork_emulation_pass`) |
| Behaviour | On wasm32, `spawn_worker` / `fork` cannot create a real isolated process (WASI has no `fork`). The fork-emulation pass rewrites parent/child control flow into a single linear-memory coroutine pair: the parent runs first, sends on its pipe, then the child runs in the *same* linear memory and receives. |
| Caveat | **There is no memory isolation between parent and child.** A bug in the "child" can corrupt the "parent"'s linear memory and vice-versa. The compiler emits a one-shot `K11A-wasm32-fork-emulation` warning at the first fork site to make this visible. |
| Mitigation | Use wasm32 only for IR-level / verification testing on host platforms where true isolation is unnecessary. For sandboxed execution, use one of the 18 native QEMU-backed backends instead. |

The fork-emulation pass is also why `try_recv` on wasm32 cannot block:
under emulation the parent always runs first, so the child's
`try_recv` either finds a buffered message immediately or returns
"empty". Blocking `recv` is lowered to a spin-loop on a flag word in
linear memory.

### 2.3 Two-pipe channel handles are 16 bytes (4 fds)

| Aspect | Detail |
|--------|--------|
| File | `src/codegen/src/ipc_lowering.rs` (see `two-pipe channel architecture` and `Closes all 4 fds in the 16-byte two-pipe handle`) |
| Layout | Each channel end is a 16-byte handle holding 4 file descriptors: parent→child pipe (read+write ends) and child→parent pipe (read+write ends). The previous single-pipe design (and its `nanosleep`-based send/recv race workaround) has been removed. |
| Implication | Send and recv touch *different* pipes, so a half-closed channel (one direction broken, the other intact) is observable: the surviving direction will continue to succeed until the program explicitly closes its end. Code that depends on "if send failed, recv will too" must check both directions independently. |
| Capacity | Each pipe is a kernel pipe with the platform's default buffer size (typically 64 KiB on Linux). Sends larger than the pipe buffer block until the reader drains; there is no per-channel buffering beyond the kernel pipe. |

### 2.4 Per-backend ABI / encoding quirks

Most ISA encoding bugs that previously appeared here have been fixed.
The remaining live per-backend quirks are tracked in
[`docs/backends.md`](backends.md) and `docs/fp_backends.md`; consult
those files for the current per-ISA matrix. Anything not listed there
should be considered a bug, not a known caveat.

---

## 3. Verification

### 3.1 `pmt-runtime-check` is a no-op at the IVE layer

| Aspect | Detail |
|--------|--------|
| Files | `src/ive/Cargo.toml` (`pmt-runtime-check = []`); `build.rs` (file-level doc, "Lean FFI bridge removed") |
| History | The feature used to wire Lean-verified PMT checkers into the runtime via a C-archive FFI bridge. That FFI bridge has been **deleted** — Z3-based contract discharge and hand-written Rust verifiers now do the work. |
| Current state | The feature is **retained as a no-op for `vuma-ive`** so existing CI commands (`cargo build --features pmt-runtime-check`) continue to work without changes. In `vuma-codegen` the feature still has a real effect: it activates the independent pure-Rust `pmt_check` module (a parity-tested hand-translation of the Lean definitions in `proof/PMT/Extraction.lean`) — but that module does **not** depend on any Lean linkage. |
| Caveat | If you enable `pmt-runtime-check` expecting Lean-verified runtime checkers, you will instead get the Rust hand-translations. They are parity-tested against the Lean definitions (see `tests/pmt_parity_test.rs`) but are not themselves formally verified. |

### 3.2 Lean proofs are a standalone artifact, not linked into the binary

| Aspect | Detail |
|--------|--------|
| Directory | `proof/` (Lean 4, pinned via `proof/lean-toolchain` to `leanprover/lean4:v4.21.0`) |
| Status | The proofs still build (`lake build`) and `scripts/check-lean.sh` still greps for `sorry`. They remain the formal specification of the PMT memory model. |
| Caveat | **The proofs are no longer linked into the compiler binary.** Build, link, and runtime verification now go through Z3 and the hand-written Rust verifiers. The Lean proofs are documentation/specification only — they do not gate the build and they do not run at compile time. |
| Implication | A `sorry` in `proof/` no longer weakens any guarantee the compiler actually delivers; it only weakens the standalone formal spec. See [`docs/pmt-formal-spec.md`](pmt-formal-spec.md) and [`docs/pmt-iris-spec.md`](pmt-iris-spec.md) for the current proof status. |

---

## 4. Testing

### 4.1 IPC test phase is worker-capped (`VUMA_IPC_WORKER_CAP`)

| Aspect | Detail |
|--------|--------|
| File | `scripts/pi5_test_suite.sh` (see `VUMA_IPC_WORKER_CAP` handling) |
| Default | 3 workers for the IPC phase, regardless of `--workers`. |
| Override | `VUMA_IPC_WORKER_CAP=N bash scripts/pi5_test_suite.sh --workers N` |
| Reason | IPC tests do `fork + exec + wait` under QEMU. Translation-cache warm-up + pipe-buffer contention under high parallelism causes sporadic fork+exec timeouts. Capping the IPC phase to ≤3 workers avoids the contention without slowing the non-IPC phases. |
| Validation | Invalid / non-integer values fall back to 3; values `<1` are floored to 1. The chosen value is logged (`[K11C] VUMA_IPC_WORKER_CAP=N …`). |

### 4.2 QEMU user-mode version

The test suite expects QEMU user-mode binaries on `$PATH` (one per
backend ISA, plus `qemu-mips64el-static` for the little-endian
`mips64` backend and `qemu-i386-static` for `x86_32`). QEMU **10.0 or
newer** is recommended. Older QEMU 7.2.0-1 static builds still work
for most backends but several previous-ISA-encoding workarounds that
targeted QEMU 7.2 bugs have been removed; if you see encoding-related
failures on an old QEMU, upgrade to 10.0+ before filing a bug.

### 4.3 wasmtime for the `wasm32` row

The `wasm32` row of the 19-backend matrix runs under `wasmtime`
(v29 or newer). The pinned version in CI is whatever is current on
the runner; older `wasmtime` (pre-v29) does not support the WASI
preview features the wasm32 backend emits and will reject the
module. See [`docs/building.md`](building.md) for install
instructions.

### 4.4 `--commit` is opt-in

`scripts/pi5_test_suite.sh` does **not** auto-commit by default. Pass
`--commit` to opt into staging the result files, writing a commit,
and pushing to `origin HEAD`. Without `--commit`, the script prints a
preview (staged files + byte sizes + proposed commit message + `git
status --porcelain`) and stops short of running `git commit` /
`git push`. `--dry-run` is the same preview without the commit step.
Flag precedence: `--no-push` → `--dry-run` → `--commit` →
default-off summary.

---

## 5. CLI surface

### 5.1 Removed flags

The following flags have been removed from the CLI and should not
appear in docs, scripts, or examples:

- `--safe` / `--no-memory-safety` — runtime bounds-check injection is
  always on; there is no flag to disable it.
- `--repl` — the interactive REPL has been removed.
- Any "Wave"-named task references — these were internal milestone
  labels and have no meaning in the current codebase.

If you find a script or doc still using one of these, delete the
reference rather than re-adding the flag.

---

## 6. Cross-references

- Build dependencies, toolchain, and profiles: [`docs/building.md`](building.md)
- Per-backend ABI / encoding matrix: [`docs/backends.md`](backends.md),
  [`docs/fp_backends.md`](fp_backends.md)
- Pipeline stages (including IVE contract discharge): [`docs/pipeline.md`](pipeline.md)
- PMT formal specification: [`docs/pmt-formal-spec.md`](pmt-formal-spec.md),
  [`docs/pmt-iris-spec.md`](pmt-iris-spec.md)
- Test-suite overview: [`docs/testing.md`](testing.md)
- Architecture: [`docs/architecture.md`](architecture.md),
  [`docs/kernel-architecture.md`](kernel-architecture.md)
