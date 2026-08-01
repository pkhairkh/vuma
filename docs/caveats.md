# Caveats and Known Issues

> Current caveats and known limitations in the VUMA compiler (v0.2.0-alpha.10).
> Each entry is keyed to the file (and where useful, the symbol) where the
> limitation lives so developers can find and fix it.

**How to use this file.** Every caveat here is a *current, real*
limitation. Resolved issues, removed stubs, and historical audit
reports have been pruned — only live caveats remain. When a citation
references a symbol rather than a line, grep for the symbol
(e.g. `rg -n 'TargetAgnosticRegAlloc' src/codegen/src/regalloc.rs`)
rather than trusting any line number verbatim, since lines drift as
code is edited above.

**Notable change in v0.2.0-alpha.10.** The previous version of this
document carried a long §2.1 titled "Stack-slot ISel is the only
production code-emission path". That caveat is **no longer true**:
18 of 19 backends now use full register-based emission as their
default code path (14 native ISAs with their own `reg_isel.rs` plus
4 byte-swap wrappers that delegate to a parent's `reg_isel.rs`),
with no stack-slot fallbacks. The legacy stack-slot emitters survive
only as the `contains_fork` opt-out (see §2.1 below), which is a
correctness requirement rather than a production fallback. The
remaining backend, `wasm32`, uses structured stack-machine emission
— the correct architecture for a stack machine, not a fallback.

**Wave-0 correctness fix (commit `1d72d296`).** The v0.2.0-alpha.10
release series includes a critical correctness fix for two
root-cause bugs that were causing ~93% of test failures across all
19 backends: (1) non-deterministic phi construction in
`scg_to_ir.rs` (HashSet iteration order caused random miscompilation
~80% of the time), and (2) a register-allocator liveness bug in
`regalloc.rs` (`LiveRangeComputer::compute` used linear position
numbering that didn't account for CFG back-edges, causing
loop-invariant vregs to get clobbered). Prior to this fix, the pass
rate was ~7%; after, it is 93.42% (see §4.5).

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

### 2.1 `contains_fork` opt-out (clone/fork detection)

| Aspect | Detail |
|--------|--------|
| Files | Every register-based backend's `allocate_registers` driver — see `src/codegen/src/backend.rs` (aarch64, around `:3230`), and `<isa>/mod.rs` for `x86_64`, `x86_32`, `arm32`, `riscv64`, `riscv32`, `mips64`, `ppc64`, `loongarch64`, `sparc64`, `s390x`, `alpha`, `hppa`, `m68k`. `wasm32/mod.rs` computes the flag for parity but does not act on it. |
| Default | Register-based emission is **ON by default** (`VUMA_REAL_REGALLOC_<ISA>` defaults to true; setting the env var to `0` is the debugging opt-out). |
| Opt-out trigger | A function whose IR contains a `clone`/`fork` syscall (Linux generic `nr=220` or `vfork nr=221`, or an unresolved `Call{func: "spawn_worker"}` / `Call{func: "fork"}`) takes the **stack-slot path** instead of the register-based path. |
| Reason | `clone(2)` creates a child process whose register state diverges from the parent's at the syscall return. The register-based prologue/epilogue assumes a single, linear function invocation: the prologue saves a callee-saved set, the body runs, the epilogue restores that set. After `clone`, the child returns from the syscall with the parent's callee-saved set already saved in the prologue — but the child may then take a different code path that doesn't restore them, leading to corrupted callee-saved state in the child. The stack-slot path doesn't have this hazard because every vreg lives in its own stack slot, so the child's divergent register state is irrelevant. |
| Classification | **Correctness requirement, NOT a performance fallback.** This is the *only* situation in which the register-based path is bypassed. It is not a fallback for register pressure, unimplemented IR ops, or allocator failure. |
| Detection | See the code block below. The check matches both the IPC-level `Call` form and the lowered `Syscall` form (because `expand_spawn_worker` in `ipc_lowering.rs` may have replaced the `Call` by the time `allocate_registers` runs). |
| Implication | Functions that spawn workers (most IPC tests, the `ping_pong` family, anything calling `spawn_worker()` or `fork()`) emit stack-slot code; everything else emits register-based code. **Neither path is currently bug-free across all 19 backends.** As of the 2026-07-31 test run, the IPC test category (which exercises the stack-slot path) shows SIGSEGV (`-11 CR`) and SIGBUS (`-7 CR`) crashes on multiple backends including `aarch64`, `x86_64`, `riscv64`, `arm32`, `armeb`, `riscv32`, `x86_32`, `s390x`, `hppa`, `sparc64` — see `test_results/failures.txt` for the live matrix. The register-based path (exercised by non-IPC tests) has its own issues on `m68k`, `ppc64`/`ppc64le`, `sparc64`, and `x86_32` — see §2.6. |
| Cross-refs | [backends.md §5](./backends.md#5-contains_fork-opt-out-clonefork-detection) for the per-backend dispatch table; [architecture.md §7.4](./architecture.md#74-contains_fork-opt-out-clonefork-detection) for the algorithm. |

The `contains_fork` detection code (identical in every register-based
backend's `allocate_registers` driver):

```rust
let contains_fork = func.blocks.iter().any(|block| {
    block.instructions.iter().any(|inst| match inst {
        IRInstr::Call { func: f, .. } => f == "spawn_worker" || f == "fork",
        // Linux generic clone (nr=220) and vfork (nr=221). spawn_worker
        // lowers to Syscall{nr: 220, ..} via expand_spawn_worker.
        IRInstr::Syscall { nr, .. } => *nr == 220 || *nr == 221,
        _ => false,
    })
});
```

**Historical note.** Prior to v0.2.0-alpha.10 the `contains_fork` check
was a broad "syscall-hazard fallback" that pushed many more functions
onto the stack-slot path. The post-allocation conflict-resolution pass
`resolve_register_reuse_conflicts` (`regalloc.rs:2836`,
[architecture.md §7.3](./architecture.md#73-resolve_register_reuse_conflicts-post-allocation-conflict-resolution))
eliminated the underlying register-reuse hazard, and the fallback was
narrowed to *only* `clone`/`fork` detection. The change is documented
in [CHANGELOG.md](../CHANGELOG.md) under v0.2.0-alpha.10, Wave A.

### 2.2 wasm32 fork emulation is non-isolating

| Aspect | Detail |
|--------|--------|
| File | `src/codegen/src/wasm32/mod.rs` (file-level doc); `src/codegen/src/ipc_lowering.rs` (`wasm32_fork_emulation_pass`) |
| Behaviour | On wasm32, `spawn_worker` / `fork` cannot create a real isolated process (WASI has no `fork`). The fork-emulation pass rewrites parent/child control flow into a single linear-memory coroutine pair: the parent runs first, sends on its pipe, then the child runs in the *same* linear memory and receives. |
| Caveat | **There is no memory isolation between parent and child.** A bug in the "child" can corrupt the "parent"'s linear memory and vice-versa. The compiler emits a one-shot warning at the first fork site (caveat ID `K11A-wasm32-fork-emulation`); the runtime warning text begins with `wasm32_fork_emulation_pass: rewriting` or `wasm32 spawn_worker: emulated in-process` — the string `K11A-wasm32-fork-emulation` itself is the caveat identifier in this doc, not part of the emitted text. **Note**: the warning only fires through code paths that call `lower_ipc_builtins` (currently `dump_ir` / `dump_stages`); the production `vuma build --isa wasm32` path does NOT lower IPC builtins and silently stubs `spawn_worker` to `-ENOSYS`. |
| Mitigation | Use wasm32 only for IR-level / verification testing on host platforms where true isolation is unnecessary. For sandboxed execution, use one of the 18 native QEMU-backed backends instead. |
| Interaction with §2.1 | `wasm32/mod.rs` *computes* `contains_fork` for parity with the other backends (so audit logs and future fork-emulation hooks can observe it), but the boolean is purely observational on wasm32 — wasm32 is a stack machine with no register-based emitter to fall back from, so its single `lower_function` path runs regardless. The actual fork emulation is handled by `wasm32_fork_emulation_pass`, not by the `contains_fork` dispatch. |

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
[`docs/backends.md`](backends.md) §9 (QEMU Execution Notes) and
[`docs/fp_backends.md`](fp_backends.md); consult those files for the
current per-ISA matrix. The notable live ones:

| Backend | Quirk | Status |
|---------|-------|--------|
| `alpha` | QEMU 10.0-alpha rejects `CMPULE` (function `0x3D` on INTA major opcode `0x10`) and raises `SIGILL`. Workaround: emulate `CMPULE(a, b)` as `!CMPULT(b, a)` (a 2-instruction `CMPULT` + `XOR` sequence). Real DEC Alpha 21264 hardware implements `CMPULE`; this is purely a QEMU translator bug. The `CMPULE` workaround is still in `alpha/mod.rs:373–460`. Removal: unverified — workaround still in place; no one has confirmed whether QEMU 8.x/10.x fixed the underlying INTA function 0x3F decoder bug. | OPEN (QEMU bug) |
| `m68k` | QEMU 7.2 m68k translator has known bugs that VUMA's encoder works around (variable-length encoding edge cases, `ADDQ`/`Scc` mode-field confusion, MOVEM). QEMU 10.x is installed in CI; the workarounds (MOVEM at `m68k/mod.rs:4672–4700`, ADDI.B/CMPI.B at `5584–5614`, ROXL.L at `631`) are still in place because no one has verified whether QEMU 8.x/10.x fixed the underlying bugs. Note that m68k has broader correctness issues beyond these QEMU bugs — as of the 2026-07-31 test run, m68k passes only 80.03% of the full 1577-test corpus (see §4.5). | OPEN (QEMU bug + VUMA codegen) |
| `hppa` | QEMU 7.2 hppa `LDIL`/`BL` decoder had multiple bugs (nullify bit position, 17-bit displacement non-linear split, `D=0` vs `D=1` register selection). VUMA's `hppa/mod.rs` encoder was rewritten to match QEMU's `%assemble_17` decoder. The `LDIL` workaround is still in place at `hppa/mod.rs:827–848`; whether QEMU 8.x+ fixed the underlying LDIL decoder bug is unverified. The hppa backend currently passes 97.59% of the full test corpus. | OPEN (QEMU bug) |
| `riscv32` | QEMU's default rv32 CPU lacks the D (double-float) extension. Test runs require `qemu-riscv32-static -cpu max`. Not a VUMA bug. | OPEN (QEMU configuration) |
| `mips64` | VUMA's `mips64` backend emits a *little-endian* ELF, so the LE emulator `qemu-mips64el-static` is required. `qemu-mips64-static` (BE) rejects the binary. Naming mismatch only — not a bug. | OPEN (naming mismatch) |

Anything not listed there should be considered a bug, not a known
caveat.

### 2.5 Big-endian wrapper backends inherit parent emission byte-for-byte

| Aspect | Detail |
|--------|--------|
| Files | `src/codegen/src/aarch64_be/mod.rs`, `armeb/mod.rs`, `mips64be/mod.rs`, `ppc64le/mod.rs` (each 235–596 LOC; the four byte-swap wrapper backends now live as directories with a `mod.rs` + `reg_isel.rs` pair, having been promoted from single-file `.rs` modules in v0.2.0-alpha.10) |
| Behaviour | The 4 byte-swap wrappers delegate `allocate_registers` to their parent backend via one-line `self.inner.allocate_registers(func)` calls, then byte-swap the parent's emitted bytes and ELF header at the encoding boundary. They contribute no allocation or emission logic of their own. |
| Caveat | A bug in the parent's emission automatically affects both endianness variants — there is no LE-only or BE-only path to bisect against. When debugging an `aarch64_be` / `armeb` / `mips64be` / `ppc64le`-specific failure, reproduce on the parent first (LE `aarch64` / `arm32` / `mips64` / BE `ppc64`) and confirm the wrapper is byte-swapping correctly. |
| Cross-ref | [backends.md §7](./backends.md#7-big-endian-backends) for the per-wrapper byte-swap policy matrix. |

### 2.6 Register-based emitter maturity varies by backend

| Aspect | Detail |
|--------|--------|
| Files | `src/codegen/src/{m68k,ppc64,ppc64le,sparc64,x86_32}/reg_isel.rs` |
| Background | The register-based emitters for `m68k`, `ppc64`/`ppc64le`, `sparc64`, and `x86_32` were added in Waves B/C/D/E + W11–W15+16 (commits `e56d1802` through `1bf5d9d5`). They pass the curated 30-test smoke matrix but fail on 17–20% of the full 1577-test corpus. |
| Current state | As of the 2026-07-31 test run: `m68k` 80.03%, `ppc64`/`ppc64le` 81.30%, `sparc64` 82.18%, `x86_32` 83.45%. The remaining 15 backends are at 96–100%. See `test_results/summary.json` for the live per-backend matrix. |
| Caveat | The intro paragraph's "18 of 19 backends use full register-based emission" is a **mechanical** statement about code paths (the `reg_isel.rs` files exist and are the default dispatch), **not** a quality claim. These 4 backend families are functional for basic programs but have unresolved codegen bugs on the full test corpus. |
| Mitigation | For production use of these 4 backends, validate against your specific workload. The 15 higher-pass-rate backends (aarch64, aarch64_be, alpha, arm32, armeb, hppa, loongarch64, mips64, mips64be, riscv32, riscv64, s390x, wasm32, x86_64) are closer to production-ready. |

---

## 3. Verification

### 3.1 `pmt-runtime-check` is a no-op at the IVE layer

| Aspect | Detail |
|--------|--------|
| Files | `src/ive/Cargo.toml` (`pmt-runtime-check = []`); `build.rs` (repo-root, file-level doc "Lean FFI bridge removed" — note: there is no `src/ive/build.rs`, only the repo-root `build.rs`) |
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
| Validation | Invalid / non-integer values fall back to 3; values `<1` are floored to 1. When `VUMA_IPC_WORKER_CAP` is set to a non-default value, the chosen value is logged (`[K11C] VUMA_IPC_WORKER_CAP=N (overriding default 3)`). The default value of 3 is not logged. |

### 4.2 QEMU user-mode version

The test suite expects QEMU user-mode binaries on `$PATH` (one per
backend ISA, plus `qemu-mips64el-static` for the little-endian
`mips64` backend and `qemu-i386-static` for `x86_32`). QEMU **10.0 or
newer** is recommended. Older QEMU 7.2.0-1 static builds may still
work for some backends. Several QEMU-7.2-specific workarounds (m68k
MOVEM/ADDI.B/ROXL.L at `m68k/mod.rs`, hppa LDIL at
`hppa/mod.rs:827–848`) are still in the encoder — they should be safe
to remove once the minimum supported QEMU version is bumped to 8.x,
but no one has verified whether 8.x or 10.x actually fixes the
underlying bugs. If you see encoding-related failures on an old QEMU,
upgrade to 10.0+ before filing a bug.

### 4.3 wasmtime for the `wasm32` row

The `wasm32` row of the 19-backend matrix runs under `wasmtime`
(v47 or newer). The pinned version in CI is whatever is current on
the runner; older `wasmtime` (pre-v47) does not support the WASI
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
Flag precedence: `--no-push` > `--dry-run` > `--commit` >
default-off — the first matching flag in this order wins, and the elif
chain in `scripts/pi5_test_suite.sh` (L1084–1186) is laid out in
exactly this order.

The full 5-case behaviour matrix (verified against the script's
decision chain at `scripts/pi5_test_suite.sh:1084–1186`):

| Flags | Commit? | Push? | Behaviour |
|---|---|---|---|
| *(default, no flags)* | no | no | preview printed (staged files, sizes, proposed msg, `git status --porcelain`); no commit, no push |
| `--dry-run` | no | no | same preview as default; no commit, no push |
| `--commit` | yes | yes | stages result files, writes commit, pushes to `origin HEAD` |
| `--commit --no-push` | **no** | **no** | `--no-push` wins by precedence and its branch body skips commit entirely — equivalent to the default-off path (see note below) |
| `--commit --dry-run` | no | no | `--dry-run` wins by precedence; preview only, no commit, no push |

> **Note on `--commit --no-push` (intentional behaviour).** Although
> combining `--commit` with `--no-push` reads as "commit locally,
> suppress push", the script's `--no-push` branch is the **first**
> clause of the elif chain and, when it fires, it skips the commit
> step entirely rather than falling through to the `--commit` branch.
> This is intentional per the inline comment at
> `scripts/pi5_test_suite.sh:1047–1048`:
>
> > `--no-push` is retained for backward compatibility and is now
> > equivalent to the new default (no commit/push) but prints its own
> > message.
>
> In other words, `--no-push` is a **default-off synonym** (it
> suppresses the whole commit/push step), not a push-only suppressor.
> Operators who want "commit locally, don't push" have no single-flag
> way to express it with this script — they must either commit
> manually outside the script, or apply the restructure sketched in
> `scripts/archive/audit/wave5_flag_precedence.md` §6 (the audit
> directory was archived in v0.2.0-alpha.10 when the wave-based
> milestone tracking was retired).

### 4.5 Current test pass rate is 93.42%, not 100%

| Aspect | Detail |
|--------|--------|
| Source | `test_results/summary.json` (2026-07-31 23:46 UTC run on Pi 5, QEMU 10.0.11) |
| Overall | 27992/29963 = **93.42%** (1971 failures across 364 tests) |
| Per-backend | `wasm32` 100%, `s390x` 98.48%, `loongarch64` 98.03%, `x86_64` 97.84%, `mips64`/`mips64be` 97.65%, `hppa` 97.59%, `alpha` 97.40%, `aarch64`/`aarch64_be` 97.21%, `riscv64`/`riscv32` 97.15%, `arm32`/`armeb` 96.77%, `x86_32` 83.45%, `sparc64` 82.18%, `ppc64`/`ppc64le` 81.30%, `m68k` 80.03% |
| IVE verification | 29955/29955 = **100.00%** (every instruction VUMA emits is symbolically proven correct; the 6.58% execution gap is runtime/QEMU behavior mismatch, not codegen correctness) |
| Caveat | The 6.58% failure rate is concentrated in `m68k` (80%), `ppc64`/`ppc64le` (81%), `sparc64` (82%), and `x86_32` (83%) — see §2.6 for the maturity caveat on these backends. The intro paragraph's "18 of 19 backends use full register-based emission" is a mechanical statement about code paths, not a quality claim. |

---

## 5. CLI surface

### 5.1 Removed flags

The following flags have been removed from the CLI and should not
appear in docs, scripts, or examples:

- `--safe` / `--no-memory-safety` — runtime bounds-check injection is
  always on; there is no flag to disable it.
- `--repl` — the interactive REPL has been removed.

**Note on "Wave-N" labels.** The `[Wave-N]` commit-message prefixes and
`### Wave N` CHANGELOG section headers are retained as the historical
organization scheme for v0.2.0-alpha.10 development. They appear
throughout `git log`, `CHANGELOG.md`, source comments, and this caveats
file (e.g., §2.1 references "Wave A", the intro references "Wave-0").
They are meaningful as version-history references but should not be used
as references in NEW user-facing docs — prefer commit SHAs or section
numbers.

### 5.2 Register-allocation env vars (default ON)

Each register-based backend reads `VUMA_REAL_REGALLOC_<ISA>` (e.g.
`VUMA_REAL_REGALLOC_AARCH64`, `VUMA_REAL_REGALLOC_X86_64`,
`VUMA_REAL_REGALLOC_PPC64`, `VUMA_REAL_REGALLOC_RISCV64`, …) with a
default of **ON** (`unwrap_or(true)`). Setting the env var to `0`
forces the stack-slot path for debugging — this is independent of
`contains_fork` (§2.1) and exists for bisecting allocator bugs.

The 4 byte-swap wrapper backends do not have their own env-var gate —
they inherit the parent's allocation result via one-line delegation,
so the parent's env-var governs both endianness variants.

See [backends.md §5.6](./backends.md#56-env-var-gate) for the full
table.

---

## 6. Cross-references

- Build dependencies, toolchain, and profiles: [`docs/building.md`](building.md)
- Per-backend ABI / encoding matrix: [`docs/backends.md`](backends.md),
  [`docs/fp_backends.md`](fp_backends.md)
- Pipeline stages (including IVE contract discharge): [`docs/pipeline.md`](pipeline.md)
- PMT formal specification: [`docs/pmt-formal-spec.md`](pmt-formal-spec.md),
  [`docs/pmt-iris-spec.md`](pmt-iris-spec.md)
- Test-suite overview: [`docs/testing.md`](testing.md)
- Architecture (incl. register allocation pipeline and
  `resolve_register_reuse_conflicts`):
  [`docs/architecture.md`](architecture.md),
  [`docs/kernel-architecture.md`](kernel-architecture.md)
