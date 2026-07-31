# Wave 4 — K11A-wasm32-fork-emulation One-Shot Warning Audit (caveat §2.2)

- **Task ID:** 4-c-test
- **Agent:** 4-c-test (sub-agent, wave 4)
- **Wave:** 4 (depends on waves 0 / 1 / 2 / 3 / 4-a-audit)
- **Caveat addressed:** §2.2 — *"The compiler emits a one-shot `K11A-wasm32-fork-emulation` warning at the first fork site when compiling a VUMA program that uses `spawn_worker`/`fork` for the wasm32 backend."*
- **Files in scope (test execution; NO source edits):**
  - `src/codegen/src/wasm32/mod.rs` (read-only — the `WASM32_FORK_WARN_ONCE` guard + warning site at L4281-4294)
  - `src/codegen/src/ipc_lowering.rs` (read-only — `lower_ipc_builtins` + `wasm32_fork_emulation_pass`)
  - `tests/gold_standard/ipc/simple_send.vuma` (1 `spawn_worker` site)
  - `tests/gold_standard/u32_arith/u32_add.vuma` (0 `spawn_worker` sites — negative control)
  - `/tmp/two_fork_sites.vuma` (2 `spawn_worker` sites — one-shot semantics probe; ephemeral test artifact, not committed)
- **DoD:**
  1. Exactly 1 `K11A-wasm32-fork-emulation` warning emitted for a program with `spawn_worker` compiled for wasm32.
  2. Zero K11A warnings for a program without `spawn_worker`.
  3. This summary markdown exists at `vuma/scripts/audit/wave4_k11a_warning.md`.

---

## 1. Warning site (static confirmation)

The `K11A-wasm32-fork-emulation` warning is documented in `docs/caveats.md` L126 as the one-shot
advisory emitted at the first fork site. Its sole emission site is
`src/codegen/src/wasm32/mod.rs:4281-4294`:

```rust
if *nr == 220 {
    // Fork emulation: return 0 (child mode) so the
    // fork_emulation_pass's CFG swap correctly routes
    // execution. ...
    WASM32_FORK_WARN_ONCE.get_or_init(|| {
        vuma_log!(warn,
            "wasm32 spawn_worker: emulated in-process via \
             CFG rewriting (parent + child run sequentially \
             in the same wasm process). NOT fork, NOT thread \
             — no isolation. See caveats.md §5 row 1."
        );
    });
    ctx.emit(WasmInstr::I64Const(0));
    ctx.pop_to_vreg(*id, WasmType::I64);
}
```

The one-shot guard is `static WASM32_FORK_WARN_ONCE: std::sync::OnceLock<()>` (declared at L71),
ensuring the warning fires at most once per `vuma` process invocation, regardless of how many
`Syscall{nr:220}` (clone) instructions the wasm32 backend lowers.

The warning text contains the markers `wasm32 spawn_worker`, `emulated in-process`, `CFG rewriting`,
and references `caveats.md §5 row 1`. The literal string `K11A-wasm32-fork-emulation` is the
warning's caveat identifier in `docs/caveats.md` (not part of the runtime text — the runtime text
is the prose above). Counting the warning is done by matching `wasm32 spawn_worker: emulated
in-process` (the unique, unambiguous prefix of the K11A advisory).

## 2. Trigger path

The warning fires inside the wasm32 backend's `lower_instruction` handler for `IRInstr::Syscall`
when `nr == 220`. The `Syscall{nr:220}` is produced by `expand_spawn_worker` (in
`ipc_lowering::lower_ipc_builtins`), which rewrites `Call{func:"spawn_worker"}` into
`Syscall{nr:220, dst:<ret>}` followed by the `if pid == 0` branch pattern. `lower_ipc_builtins`
MUST therefore run BEFORE the wasm32 backend's `allocate_registers` for the K11A code path to be
reached.

## 3. Test programs

| Program | `spawn_worker` sites | Source |
|---|---|---|
| `tests/gold_standard/ipc/simple_send.vuma` | 1 | in-tree |
| `tests/gold_standard/u32_arith/u32_add.vuma` | 0 (negative control) | in-tree |
| `/tmp/two_fork_sites.vuma` | 2 (one-shot probe) | ephemeral test artifact |

`tests/gold_standard/concurrency/*.vuma` were checked but NONE use `spawn_worker` — they were all
migrated to PMT `state_new` form (no fork sites). The 33 in-tree `.vuma` files that DO use
`spawn_worker` live under `tests/gold_standard/ipc/`; each has exactly 1 `spawn_worker` site, so
the one-shot semantics across multiple fork sites had to be probed with the synthetic
`/tmp/two_fork_sites.vuma`.

## 4. Execution

### 4a. CLI path (`vuma build --isa wasm32`) — KNOWN GAP

```bash
VUMA_LOG=1 ./target/release/vuma build --isa wasm32 \
    tests/gold_standard/ipc/simple_send.vuma -o /tmp/simple_send.wasm \
    2>&1 | tee scripts/logs/wave4_k11a_cli.log
```

Log excerpt:
```
[build] Note: targeting wasm32 via direct AST→codegen path ...
[debug] unresolved call target 'wait_worker' in wasm32 module — using stub (returns -ENOSYS)
[debug] unresolved call target 'spawn_worker' in wasm32 module — using stub (returns -ENOSYS)
Compiled tests/gold_standard/ipc/simple_send.vuma -> /tmp/simple_send.wasm (1355 bytes, ISA: wasm32)
```

**K11A warning count via CLI: 0.**

Root cause: `cmd_build_direct` (`src/main.rs:1680-1828`) calls `backend.allocate_registers(func)`
directly without first running `vuma_codegen::ipc_lowering::lower_ipc_builtins(func,
BackendKind::Wasm32)`. Consequently `spawn_worker` stays as an unresolved `Call` and is stubbed to
`-ENOSYS` by `resolve_call_relocations` (`wasm32/mod.rs:5797-5811`). The `Syscall{nr:220}` handler
that hosts the K11A warning is never reached. The same gap affects `vuma emit` and `vuma compile`
(both use `compile_to_binary_direct`). The canonical `compile_with_path` pipeline DOES call
`lower_ipc_builtins` (`pipeline.rs:1171`) but is hard-wired to `BackendKind::AArch64`, so its
`if backend == BackendKind::Wasm32 { wasm32_fork_emulation_pass(func); }` arm never fires either.

This is a **production CLI gap**, not a defect in the K11A warning mechanism itself. Fixing it
requires inserting `lower_ipc_builtins(func, BackendKind::Wasm32)` into
`compile_to_binary_direct` for the wasm32 case — a one-line source edit, explicitly OUT OF SCOPE
for this test-only task.

### 4b. Direct code path via `dump_ir` (correct invocation)

`src/bin/dump_ir.rs` is the in-tree binary that exercises the canonical lowering sequence
(parse → `bridge_ast_to_codegen_scg` → `IRBuilder::build` → **`lower_ipc_builtins(func, kind)`**
→ `backend.allocate_registers`). Running it with `wasm32` as the backend argument invokes
exactly the code path the K11A warning was designed for.

```bash
VUMA_LOG=1 ./target/release/dump_ir \
    tests/gold_standard/ipc/simple_send.vuma wasm32 \
    > scripts/logs/wave4_k11a_warning_stdout.log \
    2> scripts/logs/wave4_k11a_warning.log
```

Log excerpt (stderr):
```
[warn] wasm32_fork_emulation_pass: rewriting `if pid == 0` so parent and child run SEQUENTIALLY ...
[warn] syscall number 220 not in translation table for Wasm32 — using generic number verbatim ...
[warn] wasm32 spawn_worker: emulated in-process via CFG rewriting (parent + child run sequentially in the same wasm process). NOT fork, NOT thread — no isolation. See caveats.md §5 row 1.
```

**K11A warning count: 1.** ✅

### 4c. Negative control — `u32_add.vuma` (no `spawn_worker`)

```bash
VUMA_LOG=1 ./target/release/dump_ir \
    tests/gold_standard/u32_arith/u32_add.vuma wasm32 \
    2> scripts/logs/wave4_k11a_negative.log
```

stderr contains only optimizer debug lines (`LoopOptimizer`, `escape+effects`, `vectorize`).

**K11A warning count: 0.** ✅

### 4d. One-shot semantics — synthetic 2-fork-site program

`/tmp/two_fork_sites.vuma` (NOT committed; ephemeral test artifact) contains two sequential
`spawn_worker()` call sites in `main`. Verified the lowered IR contains exactly 2
`Syscall{nr:220}` instructions:

```bash
grep -c "nr: 220," scripts/logs/wave4_k11a_two_fork_sites_stdout.log  # → 2
```

But the K11A warning fires only once (the `WASM32_FORK_WARN_ONCE` `OnceLock` guard):

```bash
grep -c "wasm32 spawn_worker: emulated in-process" \
    scripts/logs/wave4_k11a_two_fork_sites.log  # → 1
```

**K11A warning count for 2 fork sites: 1.** ✅ One-shot semantics confirmed.

## 5. DoD Assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| Exactly 1 `K11A-wasm32-fork-emulation` warning for a program with `spawn_worker` (wasm32) | **PASS** | `wave4_k11a_warning.log`: 1 occurrence of `wasm32 spawn_worker: emulated in-process` for `simple_send.vuma` |
| Zero K11A warnings for a program without `spawn_worker` | **PASS** | `wave4_k11a_negative.log`: 0 occurrences for `u32_add.vuma` |
| Summary markdown at `vuma/scripts/audit/wave4_k11a_warning.md` | **PASS** | this file |
| One-shot semantics across multiple fork sites | **PASS** (bonus) | `wave4_k11a_two_fork_sites.log`: 1 warning despite 2 `Syscall{nr:220}` sites in the IR |

## 6. Constraint check

- No source files edited. `git status --short` (pre-commit) shows only the new audit markdown.
  The ephemeral test program `/tmp/two_fork_sites.vuma` lives outside the repo and is not
  committed.
- No push (local commit only).
- No further sub-agents spawned.
- Time budget: ~9 minutes (the bulk was source-tracing the `lower_ipc_builtins` call-site gap;
  the actual test runs were sub-second each thanks to the warm release cache from wave 4-a).

## 7. Note for orchestrator

The K11A warning mechanism is correctly implemented (one-shot `OnceLock` guard, fires at the first
`Syscall{nr:220}` lowered by the wasm32 backend, suppressed for programs without `spawn_worker`).
However, the **production CLI path** `vuma build --isa wasm32` does NOT invoke
`lower_ipc_builtins`, so the warning does not fire when a user compiles a `spawn_worker` program
for wasm32 via the CLI. The warning only fires through code paths that explicitly call
`lower_ipc_builtins(func, BackendKind::Wasm32)` before `allocate_registers` — currently only
`src/bin/dump_ir.rs` (and `src/bin/dump_stages.rs`) do this; neither `cmd_build_direct`,
`cmd_emit`, `cmd_compile`, nor `vuma::compile_to_wasm` does.

**Recommended follow-up (source edit, separate task):** add
`vuma_codegen::ipc_lowering::lower_ipc_builtins(func, backend_kind);` to
`compile_to_binary_direct` (`src/main.rs:1800-1811`, just before the `allocate_registers` loop),
gated on `backend_kind == BackendKind::Wasm32` (or unconditionally for all non-AArch64 backends to
match the canonical pipeline's behaviour). This will make the K11A warning visible through the
production CLI as documented in caveat §2.2. The same edit would also fix `spawn_worker` being
silently stubbed to `-ENOSYS` on the wasm32 direct path.

### Status: PASS (mechanism verified; CLI gap documented for follow-up)
