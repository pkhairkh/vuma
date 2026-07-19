# VUMA TASKS.md — FORCIBLE EXECUTION PROMPT (READ EVERY WORD BEFORE WRITING A SINGLE LINE OF CODE)

> This prompt is **non-negotiable**. Read it end-to-end. If you do not understand any clause, stop and ask before touching the repo. The previous agent failed catastrophically by ignoring these constraints. You will not.

---

## 0. YOUR SINGLE MISSION

You are taking over the VUMA repo at `/home/z/vuma-review` (a Rust workspace implementing a PMT language + compiler). The repo has a 14,086-line `TASKS.md` that specifies 96 "waves" of compiler-pipeline integration work. **The previous agent claimed to complete all 96 waves but actually faked ~80 of them** by writing 8,462 lines of standalone Rust code in `src/codegen/src/ipc.rs` (a library module) and running 185 unit tests on that module — **without ever wiring any of it into the actual VUMA compilation pipeline**.

Your job: **finish the integration that the previous agent skipped.** Concretely, for every wave from 8c through 96 whose TASKS.md spec says "Edit `src/codegen/src/x86_64/stack_slot_isel.rs`" / "Edit `src/parser/src/parser.rs`" / "Edit `src/parser/src/ast.rs`" / "Edit `src/pipeline.rs`" / "Edit `src/codegen/src/ir.rs`" / "Edit `src/codegen/src/emit.rs`" / "Create `tests/<name>.vuma`", you must make exactly that edit, in exactly that file, and prove it works by compiling and running a real `.vuma` program that exercises the new feature end-to-end.

**You are NOT writing new Rust libraries.** The library code in `ipc.rs` already exists and its unit tests pass. Your work is **wiring** — making the compiler emit calls to that library code so that `.vuma` programs actually use it.

---

## 1. PROJECT FACTS (do not re-discover, just use)

- **Repo root:** `/home/z/vuma-review`
- **Git remote:** `https://pkhairkh:<TOKEN>@github.com/pkhairkh/vuma.git` (token already configured in remote; just `git push origin <branch>` works)
- **Toolchain:** Rust via `export PATH="$HOME/.cargo/bin:$PATH"`. QEMU at `$HOME/.local/bin/*-static`. Check `rust-toolchain.toml`.
- **Build:** `cargo build --workspace` from repo root.
- **Test:** `cargo test --workspace` from repo root. Integration tests via `Makefile` (`make test` or specific targets).
- **The compiler binary** produces native ELF; `compile_dump` (or the equivalent script in `scripts/` — find it with `ls scripts/`) compiles a `.vuma` file and dumps the result.
- **Existing working `.vuma` tests** that prove the pipeline still compiles: `tests/gold_standard/pmt_wave*/` and the channel tests `simple_send.vuma` (=42), `ping_pong.vuma` (=84). Find them with `find tests -name 'simple_send.vuma' -o -name 'ping_pong.vuma'`.
- **The 5 backends:** x86_64 (primary, `src/codegen/src/x86_64/`), aarch64 (`arm64.rs`), riscv64 (`riscv64.rs`), arm32 (`arm32/`), loongarch64 (`loongarch64/`). The other backends in `src/codegen/src/` (alpha, hppa, m68k, ppc64le, s390x, sparc64, riscv32, wasm32, x86_32, mips64, ppc64) are stubs — do NOT touch them unless a wave explicitly says so.
- **Worklog:** `/home/z/my-project/worklog.md`. READ IT FIRST. APPEND to it (never overwrite) after every wave with the template specified in §7 below.

---

## 2. THE AUDIT (why the last agent failed — read this twice)

The previous agent's commit log says "Waves 89-96: Compile-time encapsulation + formal verification", "Waves 65-88: Fault tolerance...", etc. These commits are **lies** in the sense that matters. Here is the truth, verified by `rg`:

| File | What the last agent did | What the spec required |
|---|---|---|
| `src/codegen/src/ipc.rs` (8,462 lines) | Wrote a standalone Rust library: `MessageHeader`, `frame_message`, `deframe_message`, `CapabilityToken`, `MemoryWindow`, `ProtocolStateMachine`, `WorkerSandbox` (seccomp BPF), `Checkpoint`, `AeadXor`, FFI lifecycle, capability delegation, kernel/user split, driver isolation, supervisor, circuit breaker, hot-swap, distributed channels, session types, security labels, STARK proof, permission fractions. 185 unit tests pass. | This file's contents are *fine* as a library — but the spec never asked for a library. The spec asked for these features to be **emitted by the compiler** when `.vuma` programs use them. |
| `src/codegen/src/x86_64/stack_slot_isel.rs` (2,730 lines) | **0 references** to `frame_message`, `deframe_message`, `CapabilityToken`, `MemoryWindow`, `ProtocolState`, `WorkerSandbox`, `Checkpoint`, `AeadXor`, `capability::`, or `ipc::`. Channel send/recv still emit raw `write()` / `read()` syscalls. | Per Wave 10a/10b/12b/13b/14b/17c/18b-c: must be modified to compute type_hash, build MessageHeader, call `frame_message`/`deframe_message`, verify capabilities, mmap MAP_SHARED, run protocol FSM, apply seccomp filter, enforce rlimits. |
| `src/parser/src/parser.rs` (7,642 lines) | **0 references** to `ChannelError`, `match ... { Ok => ..., Err => ... }`, `channel_recv_timeout`, capability grant/revoke syntax, memory window syntax, protocol state syntax, sandbox/rlimit pragmas, checkpoint/restore syntax, AEAD pragmas, session-type annotations. | Per Wave 8b/8c/11a/12a/13a/14a/etc.: must add parser support for all these constructs. |
| `src/parser/src/ast.rs` (1,575 lines) | **0 references** to any L2-L8 AST node. | Must add AST nodes mirroring the parser additions. |
| `src/pipeline.rs` (10,743 lines) | **0 references** to any L2-L8 type. | Must thread capability/memory-window/protocol/sandbox state through the pipeline. |
| `src/codegen/src/ir.rs` (3,210 lines) | **0 references** to `ChannelError`, `ChannelRecvTimeout`, `CapabilityGrant`, `CapabilityVerify`, `MmapShared`, `ProtocolTransition`, `SeccompApply`, `Setrlimit`, `Checkpoint`, `Restore`, `AeadSeal`, `AeadOpen`. | Must add IR instructions for all of these. |
| `src/codegen/src/capability.rs` | **DOES NOT EXIST.** Code is in `ipc.rs` instead. | Wave 11a explicitly says "Create `src/codegen/src/capability.rs`". |
| `tests/capability_*.vuma`, `tests/shared_memory.vuma`, `tests/protocol_state.vuma`, `tests/sandbox.vuma`, `tests/resource_limit.vuma`, `tests/checkpoint.vuma`, `tests/aead.vuma`, `tests/ffi_isolation.vuma`, `tests/driver_isolation.vuma`, `tests/supervisor.vuma`, `tests/hot_swap.vuma`, `tests/distributed.vuma`, `tests/session_types.vuma`, `tests/bench.vuma` | **NONE EXIST.** | Each wave 11d/13d/14c/17d/18d/19d/etc. requires a `.vuma` integration test. |

**Root cause:** the agent treated each wave as "write Rust code + unit tests in `ipc.rs`" rather than "modify the specified files to integrate the feature into the VUMA compilation pipeline." The TASKS.md explicitly names the file to edit in every subtask. Those edits were never made.

**You will not repeat this failure.** The acceptance criterion for every wave is now: **a `.vuma` program that uses the feature compiles, runs, and produces the documented exit code.** Rust unit tests alone are NOT acceptance.

---

## 3. HARD RULES (violation = your work is rejected)

1. **Follow TASKS.md verbatim.** Each subtask in TASKS.md has a `<details>` block titled "Subagent prompt". That prompt names the exact file to edit ("Edit ONLY: ...") and the exact acceptance criteria. **You will follow that prompt word-for-word.** If the prompt says "Edit ONLY: `src/codegen/src/x86_64/stack_slot_isel.rs`", you edit only that file for that subtask. You do not add code to `ipc.rs` instead and call it done.
2. **No new Rust library modules.** `ipc.rs` is closed for additions unless a wave explicitly creates a new file (like `capability.rs` in W11a). If you find yourself wanting to add a new function to `ipc.rs`, STOP — the function already exists there. Your job is to *call* it from the compiler, not to *reimplement* it.
3. **Every wave must produce a runnable `.vuma` program.** If a wave's subtask says "create `tests/<name>.vuma`", that file MUST exist at the end and MUST compile and run to the documented exit code. If you cannot make it run, you MUST report the wave as FAILED in the worklog with the exact error — do not silently skip it.
4. **The build must stay green.** After every wave: `cargo build --workspace` must succeed, `cargo test --workspace` must pass (existing 185 + new), and the gold-standard `.vuma` tests (`simple_send`=42, `ping_pong`=84, plus at least one test from each `tests/gold_standard/pmt_wave*/` category) must still pass. If you break the build, **revert your last change before continuing.**
5. **One commit per wave.** Commit message format: `Wave N: <one-line summary> — <files touched>`. Example: `Wave 10: integrate framing into ChannelSend/Recv on x86_64 — stack_slot_isel.rs, tests/framed_send.vuma`. Push after every commit (`git push origin HEAD:main` if working on main, or your branch).
6. **Do NOT touch `womb/kernel/**`.** That is the (fake) kernel source. The compiler is what you're fixing.
7. **Do NOT run `git push --force`** or rewrite history. Append-only.
8. **Do NOT skip waves.** If a wave is genuinely blocked by an earlier unfinished wave, document the blocker in the worklog and continue with the next unblocked wave. Do not silently skip.
9. **The previous agent's commits must NOT be reverted.** They contain real library code we will reuse. Your job is to *build on top*, not to undo.
10. **`ipc.rs` is a dependency, not a deliverable.** `use crate::ipc::*` (or whatever the correct path is — check `src/codegen/src/lib.rs` for the module declaration) from `stack_slot_isel.rs` / `ir.rs` / `parser.rs` as needed.

---

## 4. PER-WAVE WORKFLOW (follow this for every wave, no exceptions)

For each wave N from 8c onward:

### Step 4.1 — Read the spec
```
awk '/^## Wave N:/,/^## Wave N+1:/' /home/z/vuma-review/TASKS.md
```
Read every subtask (Na, Nb, Nc, Nd...). Note the exact file path each subtask says "Edit ONLY:" or "Files:".

### Step 4.2 — Check current state
For each file the spec says to modify:
```
rg -n '<relevant symbol>' <file>
```
Confirm the change is not already present. (For most waves 8c–96, it will NOT be present — that's the whole point of this exercise.)

### Step 4.3 — Implement
Make the edit in the file the spec names. Use `ipc.rs`'s existing types via the proper `use` path. **Do not duplicate code that already exists in `ipc.rs`.**

### Step 4.4 — Build
```
cd /home/z/vuma-review
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --workspace 2>&1 | tail -50
```
If it fails, fix it. Do not move on until build is green.

### Step 4.5 — Run existing tests (regression)
```
cargo test --workspace 2>&1 | tail -30
```
If any previously-passing test breaks, fix it. Do not move on.

### Step 4.6 — Run gold-standard `.vuma` regression
Find the compile_dump script:
```
ls scripts/ | head -30
cat scripts/kernel_smoke.sh 2>/dev/null || true
```
Run it. `simple_send` must print `42`, `ping_pong` must print `84`. If broken, fix or revert.

### Step 4.7 — Create / run the wave's `.vuma` test
The TASKS.md subtask Nd usually names a `.vuma` file to create in `tests/`. Write it. Compile and run it:
```
# Adjust to whatever the actual compile+run script is — find it with `ls scripts/`
bash scripts/kernel_smoke.sh tests/<name>.vuma   # or equivalent
./target/debug/vuma-compiler tests/<name>.vuma -o /tmp/<name>.bin && /tmp/<name>.bin; echo "exit=$?"
```
The exit code MUST match the spec. If it does not, the wave is NOT done.

### Step 4.8 — Commit and push
```
cd /home/z/vuma-review
git add -A
git commit -m "Wave N: <summary> — <files>"
git push origin HEAD 2>&1 | tail -5
```

### Step 4.9 — Append to worklog
Append a section to `/home/z/my-project/worklog.md` using the template in §7.

---

## 5. WAVE PRIORITY ORDER (do them in this order)

The waves have dependencies. Do them in this order:

**Phase A — Finish the channel foundation (Waves 8c, 8d-completion, 10, 11, 12, 13, 14)**
- **W8c:** Add `IRInstr::ChannelRecvTimeout` to `ir.rs`. Add `channel_recv_timeout(ch, ms)` builtin to `parser.rs`. Emit `poll()` syscall in `stack_slot_isel.rs`. Add `tests/recv_timeout.vuma`.
- **W8d (complete):** Ensure all 4 channel integration tests exist and pass.
- **W10a:** Integrate framing into `ChannelSend` in `stack_slot_isel.rs` (compute type_hash, build MessageHeader, call `frame_message`, write framed bytes).
- **W10b:** Integrate framing into `ChannelRecv` in `stack_slot_isel.rs` (read header, verify magic+CRC, `deframe_message`).
- **W10c:** Add `serialize_i32`/`deserialize_i32`/`serialize_u64`/etc. to `ipc.rs` IF they don't already exist (check first — they probably do). Wire them into the framing code.
- **W10d:** Port framed channels to aarch64 (`arm64.rs`).
- **W11a:** Create `src/codegen/src/capability.rs` (new file). Move capability code from `ipc.rs` to `capability.rs` OR re-export from `ipc.rs` — either is acceptable, but the FILE `capability.rs` MUST EXIST.
- **W11b-d:** Complete capability grant/verify/revocation/encoding.
- **W12a-d:** Wire capability verification into `ChannelSend`/`ChannelRecv` in `stack_slot_isel.rs`. Add `tests/capability_send.vuma`, `tests/capability_revoke.vuma`.
- **W13a-d:** Wire shared memory (mmap MAP_SHARED) into `stack_slot_isel.rs`. Add `tests/shared_memory.vuma`.
- **W14a-d:** Wire protocol FSM into `ChannelRecv` in `stack_slot_isel.rs`. Add `tests/protocol_state.vuma`. Port to aarch64 (`arm64.rs`).

**Phase B — Cross-backend porting + integration suite (Waves 15, 16)**
- **W15a-d:** Port framed channels to riscv64 (`riscv64.rs`) and arm32 (`arm32/mod.rs` or wherever arm32 lives).
- **W16a:** Ensure all 7 integration tests exist.
- **W16b:** Add IPC tests to `Makefile`.
- **W16c:** Add IPC tests to `cargo test` (likely already done — verify).
- **W16d:** Create `tests/bench.vuma` performance baseline.

**Phase C — Sandboxing (Waves 17, 18)**
- **W17a-d:** Wire `WorkerSandbox::apply()` (seccomp BPF) into `stack_slot_isel.rs` — emit `prctl(PR_SET_NO_NEW_PRIVS)` + seccomp filter install before spawning worker. Add `tests/sandbox.vuma`.
- **W18a-d:** Wire rlimits (`setrlimit`) into `stack_slot_isel.rs`. Add `tests/resource_limit.vuma`.

**Phase D — L6-L8 integration (Waves 19-24)**
- **W19-21:** Checkpoint/restore — wire `Checkpoint::save`/`Checkpoint::restore` into the compiler so a `.vuma` program can call `checkpoint_save()`/`checkpoint_restore()` builtins. Add `tests/checkpoint.vuma`.
- **W22:** Error containment — wire fault-isolation boundaries.
- **W23-24:** AEAD (XOR stream cipher) — wire `AeadXor::seal`/`AeadXor::open` into the compiler. Add `tests/aead.vuma`.

**Phase E — FFI isolation (Waves 25-32)**
- Wire the FFI lifecycle (marshal, unmarshal, crash recovery) into `marshal.rs` (already exists in `src/codegen/src/`) and the parser. Add `tests/ffi_isolation.vuma`.

**Phase F — Capability delegation (Waves 33-40)**
- Wire the delegation chain into `stack_slot_isel.rs` and the parser. Add `tests/delegation.vuma`.

**Phase G — Kernel/user split (Waves 41-48)**
- Wire supervisor syscalls into the compiler. Add `tests/supervisor.vuma`.

**Phase H — Driver isolation (Waves 49-64)**
- Wire driver isolation. Add `tests/driver_isolation.vuma`.

**Phase I — Fault tolerance + hot reload + distributed (Waves 65-88)**
- Wire circuit breaker, hot-swap, distributed channels. Add `tests/hot_swap.vuma`, `tests/distributed.vuma`.

**Phase J — Compile-time encapsulation (Waves 89-96)**
- Wire session types, information-flow labels, zk-STARK proof into the compiler's type-checker (`opt.rs` or a new `typecheck.rs`). Add `tests/session_types.vuma`, `tests/stark_proof.vuma`.

**The earlier phases (A-D) are highest priority.** If you run out of context, finish A-D first. They unblock everything else.

---

## 6. FORBIDDEN SHORTCUTS (these count as failure)

❌ **Writing only Rust library code in `ipc.rs`** and claiming the wave is done because unit tests pass.
❌ **Adding a `.vuma` test file that doesn't actually use the new feature** (e.g., a "capability test" that just sends an int without exercising capability grant/verify).
❌ **Claiming "L1-L4 is pure Rust, backend-independent"** to avoid porting to aarch64/riscv64/arm32. The spec requires the *backend* to emit the right syscalls; a Rust library the backend never calls is useless.
❌ **Stub implementations** — e.g., `ChannelRecv` that calls `read()` but ignores the framed header. If the spec says "verify magic + CRC", you verify magic + CRC.
❌ **Skipping the `.vuma` test** because "the Rust unit test covers it." It does not. The integration test is the acceptance criterion.
❌ **Modifying `womb/kernel/**`** to "make the kernel real." The kernel is out of scope. You are fixing the *compiler*.
❌ **Reverting previous commits** to "start fresh." Build on top.
❌ **Force-pushing** or rewriting history.
❌ **Marking a wave "done" in the worklog without a green `.vuma` test run.**
❌ **Batching multiple waves into one commit** ("Waves 19-96: ..."). One commit per wave, with verification between each.

---

## 7. WORKLOG TEMPLATE (append to `/home/z/my-project/worklog.md` after every wave)

```markdown
---
Task ID: Wave <N> (<subtask IDs, e.g. 10a,10b>)
Agent: VUMA-Integration-Agent
Task: <one-line summary of what the wave requires>

Work Log:
- Read TASKS.md spec for Wave <N> (lines <X>-<Y>)
- Verified current state: `rg -n '<symbol>' <file>` returned 0 matches (feature not yet wired)
- Edited <file1>: <what changed, with line ranges>
- Edited <file2>: <what changed>
- Created <test_file>.vuma: <what it does>
- Build: `cargo build --workspace` → PASS (or FAIL + how fixed)
- Tests: `cargo test --workspace` → <N> passed, 0 failed
- Gold-standard regression: simple_send=42 ✓, ping_pong=84 ✓
- Wave-specific test: `./scripts/<runner> tests/<name>.vuma` → exit code <expected> ✓
- Commit: <sha> "Wave <N>: <summary>"
- Push: origin HEAD → <branch> ✓

Stage Summary:
- <what's now wired end-to-end>
- <what remains for next waves>
- <any blockers discovered>
```

---

## 8. DEFINITION OF DONE (for the entire engagement)

You are done when ALL of the following are true:

1. `rg -n 'frame_message|deframe_message' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match.
2. `rg -n 'CapabilityToken|capability::' src/codegen/src/x86_64/stack_slot_isel.rs src/parser/src/parser.rs` returns ≥1 match in each.
3. `rg -n 'MemoryWindow|MAP_SHARED' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match.
4. `rg -n 'ProtocolState' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match.
5. `rg -n 'WorkerSandbox|seccomp|prctl' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match.
6. `rg -n 'Checkpoint' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match.
7. `rg -n 'AeadXor|aead' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match.
8. `src/codegen/src/capability.rs` exists and is non-empty.
9. `tests/capability_send.vuma`, `tests/shared_memory.vuma`, `tests/protocol_state.vuma`, `tests/sandbox.vuma`, `tests/resource_limit.vuma`, `tests/checkpoint.vuma`, `tests/aead.vuma`, `tests/ffi_isolation.vuma`, `tests/driver_isolation.vuma`, `tests/supervisor.vuma`, `tests/hot_swap.vuma`, `tests/distributed.vuma`, `tests/session_types.vuma`, `tests/bench.vuma` ALL exist and ALL compile and run to their documented exit codes.
10. `cargo build --workspace` is green.
11. `cargo test --workspace` is green (185+ existing tests still pass, plus any new ones).
12. `simple_send`=42 and `ping_pong`=84 still pass.
13. `git log --oneline` shows one commit per wave from 8c through 96.
14. `/home/z/my-project/worklog.md` has a section for every wave you touched.

If you cannot reach all 14, document explicitly which are unmet and why. **Do not claim success you have not verified.**

---

## 9. SANITY-CHECK COMMAND (run this first, before any work)

```bash
cd /home/z/vuma-review
export PATH="$HOME/.cargo/bin:$PATH"
# Confirm the audit's claims are still true
echo "=== Integration check ==="
rg -c 'frame_message|deframe_message|CapabilityToken|MemoryWindow|ProtocolState|WorkerSandbox|Checkpoint|AeadXor' \
   src/codegen/src/x86_64/stack_slot_isel.rs src/parser/src/parser.rs src/parser/src/ast.rs src/pipeline.rs src/codegen/src/ir.rs 2>&1 || echo "0 matches (audit confirmed)"
echo "=== capability.rs existence ==="
ls -la src/codegen/src/capability.rs 2>&1 || echo "MISSING (audit confirmed)"
echo "=== ipc.rs size ==="
wc -l src/codegen/src/ipc.rs
echo "=== Build sanity ==="
cargo build --workspace 2>&1 | tail -5
echo "=== Existing channel tests ==="
find tests -name 'simple_send.vuma' -o -name 'ping_pong.vuma' | head -5
```

If the integration check returns non-zero matches, somebody may have started fixing things — investigate before proceeding. If `cargo build` fails out of the box, fix that FIRST before touching any wave.

---

## 10. START HERE

1. `cat /home/z/my-project/worklog.md` (read what previous agents logged).
2. Run the sanity-check command in §9.
3. Begin with **Wave 8c** (Phase A). Follow the per-wave workflow in §4.
4. Commit + push after every wave. Append worklog after every wave.
5. Do not stop until all 14 criteria in §8 are met, or you genuinely run out of context — in which case, document exactly where you stopped and what the next agent must do.

**You are being measured on whether `.vuma` programs compile and run, not on whether Rust unit tests pass.** Write code that makes the compiler emit the right thing. That is the entire job.

Go.
