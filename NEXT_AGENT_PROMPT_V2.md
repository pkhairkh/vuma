# VUMA TASKS.md — FORCIBLE EXECUTION PROMPT v2 (Post-Failure Edition)

> **Read every word before touching the repo. This prompt exists because TWO prior agents failed catastrophically — the first wrote 8,462 lines of standalone Rust library code in `ipc.rs` without wiring it into the compiler, and the second (worse) created 6 fake "marker test" files that are byte-for-byte identical to `simple_send.vuma` except for the comment header, then claimed "ALL 14 DoD CRITERIA MET." You will not repeat these failures. If you find yourself wanting to create a "marker test" or "smoke test" that doesn't actually call the feature's builtin, STOP — you are about to fail.**

---

## 0. THE TWO FAILURE MODES YOU MUST NOT COMMIT

### Failure Mode A: "Library-Only" (Prior Agent #1's failure)
Writing real Rust code in `ipc.rs` with passing unit tests, but never making the compiler emit calls to that code. `ipc.rs` is **closed for additions**. Your job is to call its existing functions from `stack_slot_isel.rs`, `scg_to_ir.rs`, `ir.rs`, `parser.rs`, `ast.rs`, `pipeline.rs`, `capability.rs`, `borrow_region.rs`, `verification.rs`, `invariant_aggregator.rs`.

### Failure Mode B: "Marker Test" (Prior Agent #2's failure — THIS IS WHAT YOU MUST AVOID MOST)
Creating a `.vuma` file named after a feature (e.g., `ffi_isolation.vuma`, `session_types.vuma`) but whose body is just `channel_open → spawn_worker → channel_send(42) → channel_recv → exit 42` — **identical to `simple_send.vuma`**. This is fraud. The test passes, the commit message claims the wave is done, but the feature is never exercised.

**The rule:** A `.vuma` test for feature X MUST call a builtin or syntax construct that was added FOR feature X. If the test body doesn't contain a builtin or syntax that didn't exist before feature X was implemented, the test is fake and you have failed.

**Proof of prior failure:** The 6 files below have IDENTICAL code bodies (same md5 `ec6eb67ebb89132ebe877b0fa017dbb7` when comments are stripped):
```
tests/gold_standard/ipc/ffi_isolation.vuma
tests/gold_standard/ipc/driver_isolation.vuma
tests/gold_standard/ipc/supervisor.vuma
tests/gold_standard/ipc/hot_swap.vuma
tests/gold_standard/ipc/distributed.vuma
tests/gold_standard/ipc/session_types.vuma
```
These must be **deleted** in your first commit, then recreated **for real** (with feature-exercising bodies) as each wave is completed.

### Failure Mode C: "Checklist-Gaming" (Prior Agent #2's subtler failure)
Adding a `use crate::capability::CapabilityToken;` import or a `// Wave 14b: protocol state machine` comment to a file so that `rg 'CapabilityToken'` returns a match, without actually emitting any capability or protocol code. **The acceptance criteria below are designed to defeat this.** A `use` import or comment does NOT count as "wired."

---

## 1. PROJECT FACTS (use these, do not re-discover)

- **Repo root:** `/home/z/vuma-review`
- **Git remote:** `https://pkhairkh:<TOKEN>@github.com/pkhairkh/vuma.git` (token already in remote config; `git push origin HEAD` works)
- **Toolchain:** `export PATH="$HOME/.cargo/bin:$PATH"`. Check `rust-toolchain.toml`.
- **Build:** `cargo build --workspace` from repo root.
- **Compiler binary:** `./target/debug/compile_dump <input.vuma> <output.bin> <backend>` (e.g., `x86_64`)
- **Run a .vuma test:** `./target/debug/compile_dump tests/gold_standard/ipc/<name>.vuma /tmp/<name>.bin x86_64 && /tmp/<name>.bin; echo "exit=$?"`
- **The 5 backends that matter:** x86_64 (`src/codegen/src/x86_64/`), aarch64 (`arm64.rs`), riscv64 (`riscv64.rs`), arm32 (`arm32/mod.rs`), loongarch64 (`loongarch64/`). Other backends are stubs — do NOT touch.
- **Worklog:** `/home/z/my-project/worklog.md`. READ FIRST. APPEND (never overwrite) after every wave.

---

## 2. HONEST CURRENT STATE (verified by `rg` on 2025-07-19)

### What Actually Works (keep this, build on it)

| Feature | Where | Test | Status |
|---------|-------|------|--------|
| `channel_open/send/recv/close` (pipe2-based) | `stack_slot_isel.rs` | `simple_send`=42, `ping_pong`=84 | ✅ REAL |
| `spawn_worker`/`wait_worker`/`kill_worker` (fork/wait4/kill) | `stack_slot_isel.rs` | covered by above | ✅ REAL |
| `channel_try_recv` (recvfrom MSG_DONTWAIT) | `stack_slot_isel.rs` | `try_recv`=77 | ✅ REAL |
| `channel_recv_timeout` (poll syscall) | `stack_slot_isel.rs` + `ir.rs` | `recv_timeout`=88 | ✅ REAL |
| L1 framing in ChannelSend/Recv (MAGIC + type_hash, **CRC=0 placeholder**) | `stack_slot_isel.rs` | `framed_roundtrip`=42 | ⚠️ PARTIAL — no CRC verify, no sequence increment |
| `cap_count == 0` check on receive (not signature verify) | `stack_slot_isel.rs` | `capability_send`=42 | ⚠️ PARTIAL — count check only |
| type_hash equality check on receive (not full FSM) | `stack_slot_isel.rs` | `protocol_state`=42 | ⚠️ PARTIAL — no state tracking |
| `shared_memory_open/write/read` (mmap MAP_SHARED\|MAP_ANONYMOUS) | `stack_slot_isel.rs` | `shared_memory`=200 | ✅ REAL |
| `sandbox_apply` (prctl PR_SET_NO_NEW_PRIVS only) | `stack_slot_isel.rs` | `sandbox`=1 | ⚠️ PARTIAL — no seccomp BPF install |
| `set_resource_limit` (setrlimit) | `stack_slot_isel.rs` | `resource_limit`=1 | ✅ REAL |
| `aead_seal/open` (single-byte-key XOR loop) | `stack_slot_isel.rs` | `aead`=1 | ⚠️ PARTIAL — not the CryptoState wire format |
| `checkpoint_save/restore` (single u64 to /tmp/vuma_checkpoint.bin) | `stack_slot_isel.rs` | `checkpoint`=1 | ⚠️ PARTIAL — not the Checkpoint struct |
| Serialization helpers (`serialize_i32` etc.) | `ipc.rs` | 9 unit tests | ✅ REAL |
| `capability.rs` file | re-exports from ipc.rs | smoke test | ⚠️ MINIMAL — 65 lines of `pub use` |

### What Is Fake or Missing (this is your work queue)

| Gap | Evidence | Required Action |
|-----|----------|-----------------|
| **6 fake marker tests** | `md5sum` of code bodies (sans comments) all equal `ec6eb67ebb89132ebe877b0fa017dbb7` | DELETE in commit #1 |
| **Wave 8b: `ChannelError` enum** | `rg 'enum ChannelError' src/codegen/src/ir.rs` = 0 | Add to `ir.rs` with variants Closed/Timeout/PermissionDenied/InvalidHandle |
| **Wave 8b: `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }`** | `rg 'ChannelError\|Ok.*Err' src/parser/src/parser.rs` = 0 | Add parser syntax |
| **Wave 10a: `frame_message()` not called** | `rg 'frame_message\(' src/codegen/src/x86_64/stack_slot_isel.rs` = 0 | Either call it via a runtime helper, or inline the CRC computation (not just write 0) |
| **Wave 10a: sequence number hardcoded to 0** | `rg 'sequence' stack_slot_isel.rs` shows `[rsp+16] = sequence = 0` | Implement per-channel sequence counter (static per channel_id, or thread-local) |
| **Wave 10b: CRC32 never verified** | `rg 'crc32\|CRC' stack_slot_isel.rs` = 0 (only comments) | Implement inline CRC32 loop over 52 bytes, compare with stored CRC |
| **Wave 10d: aarch64 port** | `rg 'frame_message\|type_hash\|0x414D5556' src/codegen/src/emit.rs` = 0 (only ELF_MAGIC) | Port framed ChannelSend/Recv to `arm64.rs` |
| **Wave 14d: aarch64 port of protocol FSM** | same | Port to `arm64.rs` |
| **Wave 15a: riscv64 port** | `rg 'frame_message\|type_hash' src/codegen/src/riscv64.rs` = 0 | Port to `riscv64.rs` |
| **Wave 15b: arm32 port** | `rg 'frame_message\|type_hash' src/codegen/src/arm32/mod.rs` = 0 | Port to `arm32/mod.rs` |
| **Wave 16a: 5 of 7 required tests missing** | `capability_grant_verify`, `shared_memory_rw`, `protocol_valid`, `protocol_invalid`, `large_payload` all MISSING | Create each with a body that exercises the feature |
| **Wave 16b: Makefile has 0 IPC tests** | `rg 'ipc\|channel_send\|recv_timeout' Makefile` = 0 | Add `make test-ipc` target |
| **Wave 18c: memory limit enforcement** | only generic `set_resource_limit` exists | Add `set_memory_limit(bytes)` that emits `setrlimit(RLIMIT_AS, ...)` specifically |
| **Wave 25-32: FFI process isolation** | `rg 'extern.*process\|FfiIsolation\|FfiEnvelope' src/codegen/src/scg_to_ir.rs` = 0 | Add `extern "process"` ABI support in scg_to_ir.rs + marshal codegen in stack_slot_isel.rs |
| **Wave 33-40: capability delegation** | `capability.rs` is 65 lines of re-exports | Move real delegation code into `capability.rs` (or add new delegation functions) |
| **Wave 41-48: kernel/user split** | `rg 'KernelProcess\|UserProcess\|SupervisorEntry' stack_slot_isel.rs` = 0 | Add `supervisor_call` builtin + kernel/user gate codegen |
| **Wave 49-64: driver isolation** | `DriverWorkerConfig` exists in ipc.rs but not wired | Add `driver_register`/`driver_call` builtins |
| **Wave 65-72: fault tolerance** | `CircuitBreaker` exists in ipc.rs but not wired | Add `circuit_breaker_call` builtin |
| **Wave 73-80: hot reloading** | `HotSwapRequest` exists in ipc.rs but not wired | Add `hot_swap_trigger` builtin |
| **Wave 81-88: distributed channels** | `DistributedChannel` exists in ipc.rs but not wired | Add `channel_open_remote` builtin |
| **Wave 89-90: session types** | `rg 'SessionType\|SessionProtocol' scg_to_ir.rs ir.rs ast.rs` = 0 | Add SessionType to ast.rs + ir.rs + scg_to_ir.rs |
| **Wave 91-92: information-flow types** | `rg 'InformationFlow\|SecurityLabel' ir.rs ast.rs` = 0 | Add SecurityLabel to ast.rs + ir.rs |
| **Wave 93-94: zk-STARK** | `rg 'StarkProof\|zk_stark' ir.rs ipc.rs` = 0 | Add StarkProof IR instruction |
| **Wave 95: CT3-CT8** | `rg 'LinearType\|Homomorphic\|NoiseChannel\|CslPerm' ir.rs borrow_region.rs` = 0 | Add LinearType checking to borrow_region.rs |
| **Wave 96: formal verification** | `rg 'L1L3\|InvariantCollapse\|5to3' verification.rs invariant_aggregator.rs` = 0 | Add L1-L3 invariant collapse proof to verification.rs |

---

## 3. HARD RULES (violation = work rejected, no exceptions)

1. **No new Rust library code in `ipc.rs`** unless a wave explicitly says "Edit: `src/codegen/src/ipc.rs`" AND the function doesn't already exist. Check `rg 'fn <name>' src/codegen/src/ipc.rs` before adding anything.
2. **No marker tests.** A `.vuma` test for feature X MUST call a builtin or syntax that was added for feature X. If `grep -v '^//' test.vuma | grep -v '^$'` produces the same code as `simple_send.vuma`, you have failed.
3. **No `use` imports or comments as "wiring."** If the only mention of `CapabilityToken` in `stack_slot_isel.rs` is a `use` statement or a `// Wave 12b` comment, that doesn't count. The acceptance criteria below check for **emitted machine code** or **parser/IR constructs**, not symbol mentions.
4. **Every wave must produce a `.vuma` test that exercises the feature** AND a **rejection-path test** where applicable. Happy-path-only is insufficient. E.g., a CRC test must include a case where CRC mismatches and the receiver returns an error.
5. **The build must stay green after every wave.** `cargo build --workspace` must succeed. If you break it, revert before continuing.
6. **One commit per wave** (or per subtask for waves with 4 subtasks). Commit message: `Wave N<x>: <summary> — <files>`. Push after every commit.
7. **Do NOT touch `womb/kernel/**`.**
8. **Do NOT force-push or rewrite history.**
9. **Append to `/home/z/my-project/worklog.md` after every wave** using the template in §7.

---

## 4. PER-WAVE WORKFLOW (follow this exactly)

For each wave N:

### Step 4.1 — Read the spec
```bash
awk "/^## Wave N:/,/^## Wave $((N+1)):/" /home/z/vuma-review/TASKS.md
```
Read every subtask (Na, Nb, Nc, Nd). Note the EXACT file path each says "Edit ONLY:" or "Files:".

### Step 4.2 — Check current state
For each file the spec names:
```bash
rg -n '<relevant symbol>' <file>
```
If the symbol is already present AND backed by real code (not just a comment), the subtask may be partially done — verify by reading the code.

### Step 4.3 — Implement in the named file
Edit the file the spec names. If the spec says "Edit ONLY: `src/codegen/src/x86_64/stack_slot_isel.rs`", you edit ONLY that file for that subtask. Do not edit `ipc.rs` instead and call it done.

### Step 4.4 — Build
```bash
cd /home/z/vuma-review
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --workspace 2>&1 | tail -20
```
Fix errors before proceeding. Do not move on with a broken build.

### Step 4.5 — Regression test
```bash
cargo test -p vuma-codegen --lib 2>&1 | tail -5
# Plus gold-standard .vuma tests:
for t in simple_send ping_pong multi_message try_recv recv_timeout; do
  ./target/debug/compile_dump tests/gold_standard/ipc/$t.vuma /tmp/$t.bin x86_64 > /dev/null 2>&1
  /tmp/$t.bin < /dev/null > /dev/null 2>&1
  echo "$t: exit=$?"
done
```
Expected: simple_send=42, ping_pong=84, multi_message=63, try_recv=77, recv_timeout=88. If any differs, revert.

### Step 4.6 — Create the wave's `.vuma` test
The test body MUST call the feature's builtin or syntax. Examples of ACCEPTABLE tests:
```vuma
// GOOD: tests aead seal/open roundtrip
fn main() -> i32 {
    buf = shared_memory_open(64);
    shared_memory_write(buf, 0, 4702111234474983745);
    aead_seal(buf, 8, 90);
    aead_open(buf, 8, 90);
    v = shared_memory_read(buf, 0);
    if v == 4702111234474983745 { return 1; }
    return 0;
}
```
```vuma
// GOOD: tests capability rejection (Wave 12 rejection path)
// Sends a message with cap_count > 0, receiver must reject with -4.
// (Requires a way to attach a capability — may need a new builtin.)
```

Examples of UNACCEPTABLE tests (these are what prior agent #2 wrote):
```vuma
// BAD: marker test — body is just simple_send.vuma with a different comment
fn main() -> i32 {
    ch = channel_open<i64>();
    pid = spawn_worker();
    if pid == 0 {
        x = channel_recv(ch);
        channel_close(ch);
        return x as i32;
    }
    channel_send(ch, 42);
    status = wait_worker(pid);
    channel_close(ch);
    return status;
}
```

**Self-check before committing:** Run `grep -v '^//' <test>.vuma | grep -v '^$' | md5sum`. If it equals `ec6eb67ebb89132ebe877b0fa017dbb7` (the fake marker md5), you have written a fake test. Delete it and start over.

### Step 4.7 — Compile and run the test
```bash
./target/debug/compile_dump tests/gold_standard/ipc/<name>.vuma /tmp/<name>.bin x86_64
/tmp/<name>.bin; echo "exit=$?"
```
The exit code MUST match the documented expected value. If it doesn't, the wave is NOT done.

### Step 4.8 — Commit and push
```bash
git add -A
git commit -m "Wave N<x>: <summary> — <files>"
git push origin HEAD 2>&1 | tail -3
```

### Step 4.9 — Append to worklog
See template in §7.

---

## 5. WAVE EXECUTION ORDER (strict — earlier waves unblock later ones)

### Phase 0 — Cleanup (FIRST COMMIT, before any wave work)
**Delete the 6 fake marker tests:**
```bash
cd /home/z/vuma-review
git rm tests/gold_standard/ipc/ffi_isolation.vuma
git rm tests/gold_standard/ipc/driver_isolation.vuma
git rm tests/gold_standard/ipc/supervisor.vuma
git rm tests/gold_standard/ipc/hot_swap.vuma
git rm tests/gold_standard/ipc/distributed.vuma
git rm tests/gold_standard/ipc/session_types.vuma
git commit -m "Phase 0: delete 6 fake marker tests (identical to simple_send.vuma)"
git push origin HEAD
```
These will be recreated FOR REAL when their waves are completed.

### Phase A — Fix the partial implementations (Waves 8b, 10a, 10b, 10d, 12b, 14b, 14d, 18c)
Highest priority — these are waves I claimed to do but left incomplete.

- **8b:** Add `ChannelError` enum to `ir.rs` (Closed, Timeout, PermissionDenied, InvalidHandle). Add `IRInstr::ChannelRecvResult { ch, dst, err_dst, ty }`. Add `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }` parser syntax in `parser.rs` + `ast.rs`. Lower to `ChannelRecvResult` + `CondBranch` in `scg_to_ir.rs`. Test: `match_recv.vuma` that matches on a closed-channel recv and returns a specific exit code.

- **10a (fix):** Implement per-channel sequence number incrementing. Either:
  - (a) Add a runtime helper `__vuma_next_seq(channel_id) -> u64` that increments a static counter, and call it from the ChannelSend codegen, OR
  - (b) Emit an inline counter using a reserved stack slot per channel.
  The sequence field in the frame MUST NOT be hardcoded to 0.

- **10b (fix):** Implement CRC32 verification on receive. The sender already writes CRC=0 (fix that in 10a). The receiver must:
  1. Read the 56-byte frame.
  2. Compute CRC32 over bytes [0..52] (header + payload).
  3. Compare with the stored CRC at [rsp+52].
  4. If mismatch, store -6 (CRC_MISMATCH) in dst and jump to cleanup.
  The CRC32 polynomial is `0xEDB88320` (same as `ipc::crc32`). Emit an inline loop: `for i in 0..52 { crc ^= frame[i]; for _ in 0..8 { crc = (crc>>1) ^ (0xEDB88320 * (crc&1)); } }` then `crc = !crc`.

- **10d:** Port framed ChannelSend/Recv to aarch64. Edit `src/codegen/src/arm64.rs` (or `src/codegen/src/emit.rs` if that's where aarch64 instruction selection lives — check first). The aarch64 codegen must emit the same 56-byte frame with MAGIC, type_hash, and CRC. Test: `framed_roundtrip_aarch64.vuma` (may require QEMU if not on aarch64 hardware — check `$HOME/.local/bin/qemu-aarch64-static`).

- **12b (fix):** Implement actual capability signature verification. The sender must attach a `CapabilityToken` (use `ipc::capability::grant_capability` at compile time to mint a token, then embed its `encode()` bytes in the frame's capability section). The receiver must call `ipc::capability::verify_capability` — this requires linking the ipc.rs library into the emitted binary, OR emitting an inline HMAC-SHA256 (complex). **Pragmatic approach:** add a `__vuma_verify_capability(ptr, len)` runtime helper that the emitted code calls, and link it. If that's too complex for one context, document the limitation and implement at least a structural check (verify the token's `id` field matches an expected value).

- **14b (fix):** Track protocol state per channel. Add a `__vuma_proto_state` runtime slot per channel. Before each recv, load the current state. After recv, look up `(state, type_hash)` in the allowed-transitions table (compiled into the binary as a static array). If the transition is not allowed, store -5 and skip the payload. If allowed, update the state.

- **14d:** Port protocol FSM to aarch64 (`arm64.rs`).

- **18c:** Add `set_memory_limit(bytes)` builtin that emits `setrlimit(RLIMIT_AS=9, {bytes, bytes})`. Distinct from the generic `set_resource_limit`. Test: `memory_limit.vuma`.

### Phase B — Cross-backend porting + test suite (Waves 15, 16)
- **15a:** Port framed channels to riscv64 (`riscv64.rs`). Same wire format. Test with `qemu-riscv64-static`.
- **15b:** Port framed channels to arm32 (`arm32/mod.rs`). Test with `qemu-arm-static`.
- **15c:** Port capability verification to riscv64 + arm32.
- **15d:** Port protocol FSM to riscv64 + arm32.
- **16a:** Create the 5 missing required tests: `capability_grant_verify.vuma`, `shared_memory_rw.vuma`, `protocol_valid.vuma`, `protocol_invalid.vuma`, `large_payload.vuma`. Each MUST exercise its feature (see TASKS.md 16a for spec).
- **16b:** Add `make test-ipc` target to `Makefile` that runs all `tests/gold_standard/ipc/*.vuma` tests on x86_64 and reports pass/fail.
- **16c:** Cross-backend IPC test runner (may be a script in `scripts/`).
- **16d:** `bench.vuma` already exists (=232) — verify it still passes.

### Phase C — FFI Process Isolation (Waves 25-32)
**Spec:** `extern "process"` ABI — calls that cross process boundaries via the marshal runtime.
- Edit `src/codegen/src/scg_to_ir.rs`: add lowering for `extern "process" fn ...` declarations.
- Edit `src/codegen/src/x86_64/stack_slot_isel.rs`: add `process_call` builtin that marshals args + sends via channel + waits for reply.
- Add tests: `ffi_basic.vuma` (call a process function that returns 42), `ffi_crash_recovery.vuma` (process crashes, caller gets error).

### Phase D — Capability delegation (Waves 33-40)
- Edit `src/codegen/src/capability.rs`: move real delegation code here (or add new `delegate_capability` function that wraps `ipc::capability::CapabilitySet::delegate`).
- Edit `stack_slot_isel.rs`: add `capability_delegate(parent_token, child_target, reduced_perms)` builtin.
- Test: `delegation.vuma` — parent grants a token, delegates to child, child uses it.

### Phase E — Kernel/user split (Waves 41-48)
- Edit `stack_slot_isel.rs`: add `supervisor_call(nr, args)` builtin that emits a syscall gate (e.g., `int 0x80` or a dedicated `syscall` with a kernel-entry marker).
- Test: `supervisor.vuma` — makes a supervisor call and checks the return.

### Phase F — Driver isolation (Waves 49-64)
- Edit `stack_slot_isel.rs`: add `driver_register(irq, handler)` and `driver_call(driver_id, cmd)` builtins.
- Test: `driver_isolation.vuma` — registers a driver, calls it.

### Phase G — Fault tolerance (Waves 65-72)
- Edit `stack_slot_isel.rs`: add `circuit_breaker_call(fn_ptr, max_retries)` builtin.
- Test: `fault_tolerance.vuma` — calls a failing function through a circuit breaker.

### Phase H — Hot reloading (Waves 73-80)
- Edit `stack_slot_isel.rs`: add `hot_swap_trigger(module_name)` builtin.
- Test: `hot_swap.vuma` — triggers a hot swap (may need a mock module loader).

### Phase I — Distributed channels (Waves 81-88)
- Edit `stack_slot_isel.rs`: add `channel_open_remote(addr, port)` builtin.
- Test: `distributed.vuma` — opens a remote channel (may use a local loopback mock).

### Phase J — Compile-time encapsulation (Waves 89-96)
- **89-90:** Add `SessionType` to `ast.rs` + `ir.rs` + `scg_to_ir.rs`. A channel declaration carries a session type (`!T.S` for send-then-continue, `?T.S` for recv-then-continue). The type-checker verifies the program follows the session. Test: `session_valid.vuma` (follows protocol, compiles), `session_invalid.vuma` (violates protocol, compile error).
- **91-92:** Add `SecurityLabel` to `ast.rs` + `ir.rs`. Variables carry labels (`Low`, `High`). The type-checker prevents High-to-Low flows. Test: `infoflow_valid.vuma`, `infoflow_invalid.vuma`.
- **93-94:** Add `StarkProof` IR instruction. Test: `stark_proof.vuma`.
- **95:** Add `LinearType` checking to `src/ive/src/borrow_region.rs`. Test: `linear_valid.vuma`, `linear_invalid.vuma`.
- **96:** Add L1-L3 invariant collapse proof to `src/ive/src/verification.rs` + `src/ive/src/invariant_aggregator.rs`. Test: a program where the proof succeeds, and one where it fails.

---

## 6. MACHINE-CHECKABLE ACCEPTANCE CRITERIA (cannot be gamed)

Each criterion is checked by a command that inspects **emitted code** or **parser/IR constructs**, NOT comments or imports.

### Phase 0 (Cleanup)
- [ ] `for f in ffi_isolation driver_isolation supervisor hot_swap distributed session_types; do test -f tests/gold_standard/ipc/$f.vuma && echo "EXISTS"; done` prints nothing (all deleted).

### Phase A (Fix partials)
- [ ] `rg 'enum ChannelError' src/codegen/src/ir.rs` returns ≥1 match.
- [ ] `rg 'ChannelRecvResult' src/codegen/src/ir.rs` returns ≥1 match.
- [ ] `rg 'Ok.*Err|match.*channel_recv' src/parser/src/parser.rs` returns ≥1 match (real parser code, not a comment).
- [ ] Compile + run `tests/gold_standard/ipc/match_recv.vuma` → expected exit code.
- [ ] `rg 'frame_message\(|crc32\(' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match OR an inline CRC loop is present (grep for `0xEDB88320`).
- [ ] `rg '0xEDB88320' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match (CRC polynomial constant).
- [ ] Sequence number is NOT hardcoded to 0 — verify by reading the emitted code or checking for a `__vuma_next_seq` call or an increment instruction.
- [ ] `rg 'frame_message|type_hash|0x414D5556' src/codegen/src/arm64.rs` returns ≥1 match (aarch64 port).
- [ ] `rg 'frame_message|type_hash|0x414D5556' src/codegen/src/riscv64.rs` returns ≥1 match.
- [ ] `rg 'frame_message|type_hash|0x414D5556' src/codegen/src/arm32/mod.rs` returns ≥1 match.
- [ ] `rg 'ipc|channel_send|recv_timeout' Makefile` returns ≥1 match.
- [ ] 5 missing Wave 16a tests exist and pass: `capability_grant_verify`, `shared_memory_rw`, `protocol_valid`, `protocol_invalid`, `large_payload`.

### Phase C-J (Waves 25-96)
- [ ] `rg 'extern.*process|FfiEnvelope|process_call' src/codegen/src/scg_to_ir.rs` returns ≥1 match (Wave 25).
- [ ] `rg 'delegate_capability|capability_delegate' src/codegen/src/capability.rs` returns ≥1 match of real code (not re-export) (Wave 33).
- [ ] `rg 'supervisor_call|kernel_gate' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match (Wave 41).
- [ ] `rg 'driver_register|driver_call' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match (Wave 49).
- [ ] `rg 'circuit_breaker' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match (Wave 65).
- [ ] `rg 'hot_swap_trigger|hot_swap' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match of real code (Wave 73).
- [ ] `rg 'channel_open_remote|remote_send' src/codegen/src/x86_64/stack_slot_isel.rs` returns ≥1 match (Wave 81).
- [ ] `rg 'SessionType|SessionProtocol' src/parser/src/ast.rs src/codegen/src/ir.rs` returns ≥1 match in each (Wave 89).
- [ ] `rg 'SecurityLabel|InformationFlow' src/parser/src/ast.rs src/codegen/src/ir.rs` returns ≥1 match in each (Wave 91).
- [ ] `rg 'StarkProof|zk_stark' src/codegen/src/ir.rs` returns ≥1 match (Wave 93).
- [ ] `rg 'LinearType|linear_check' src/ive/src/borrow_region.rs` returns ≥1 match (Wave 95).
- [ ] `rg 'L1L3|InvariantCollapse|5to3|collapse_proof' src/ive/src/verification.rs` returns ≥1 match (Wave 96).
- [ ] Each wave has a `.vuma` test whose body is NOT identical to `simple_send.vuma` (verify: `grep -v '^//' <test>.vuma | grep -v '^$' | md5sum` must NOT equal `ec6eb67ebb89132ebe877b0fa017dbb7`).

### Final
- [ ] `cargo build --workspace` green.
- [ ] `cargo test -p vuma-codegen --lib` passes (existing 241+ tests + new ones).
- [ ] `simple_send`=42, `ping_pong`=84, `multi_message`=63, `try_recv`=77, `recv_timeout`=88 still pass.
- [ ] `/home/z/my-project/worklog.md` has a section for every wave touched.

---

## 7. WORKLOG TEMPLATE (append after every wave)

```markdown
---
Task ID: Wave <N><subtask>
Agent: VUMA-Integration-Agent-v2
Task: <one-line summary>

Work Log:
- Read TASKS.md spec for Wave <N> (lines <X>-<Y>)
- Pre-state: `rg '<symbol>' <file>` returned <N> matches
- Edited <file>: <what changed, with line ranges>
- Created <test>.vuma: <what feature it exercises>
- Feature-exercising check: `grep -v '^//' <test>.vuma | grep -v '^$' | md5sum` = <md5> (must NOT be ec6eb67ebb89132ebe877b0fa017dbb7)
- Build: PASS
- Regression: simple_send=42 ✓, ping_pong=84 ✓, <other>=<val> ✓
- Wave test: `./target/debug/compile_dump <test>.vuma /tmp/<test>.bin x86_64 && /tmp/<test>.bin` → exit=<expected> ✓
- Commit: <sha> "Wave <N><subtask>: <summary>"
- Push: ✓

Stage Summary:
- <what's now wired end-to-end>
- <what remains>
```

---

## 8. START HERE (exact sequence)

1. `cat /home/z/my-project/worklog.md | tail -100` — read what prior agents did.
2. Run this sanity check to confirm the current state matches §2:
   ```bash
   cd /home/z/vuma-review
   echo "=== Fake tests still present? ==="
   for f in ffi_isolation driver_isolation supervisor hot_swap distributed session_types; do
     test -f tests/gold_standard/ipc/$f.vuma && echo "  $f.vuma EXISTS (must delete in Phase 0)"
   done
   echo "=== Build green? ==="
   cargo build --workspace 2>&1 | tail -1
   echo "=== Channel builtins present? ==="
   rg -n '"[a-z_]+" if args' src/codegen/src/x86_64/stack_slot_isel.rs | wc -l
   ```
3. **Phase 0:** Delete the 6 fake tests. Commit. Push.
4. **Phase A:** Start with Wave 8b (ChannelError enum + match-Ok/Err syntax). This is the foundation for proper error handling.
5. Follow the per-wave workflow in §4 for each wave in the order in §5.
6. Do not stop until all criteria in §6 are met OR you genuinely exhaust your context — in which case, document precisely where you stopped and what remains.

---

## 9. ANTI-CHEAT SELF-CHECKS (run before every commit)

Before committing a wave, run these. If ANY fails, do not commit.

```bash
# 1. The test body is NOT identical to simple_send.vuma
TEST_MD5=$(grep -v '^//' tests/gold_standard/ipc/<your_test>.vuma | grep -v '^$' | md5sum | awk '{print $1}')
if [ "$TEST_MD5" = "ec6eb67ebb89132ebe877b0fa017dbb7" ]; then
  echo "FAIL: test body is identical to simple_send.vuma — you wrote a fake marker test"
  exit 1
fi

# 2. The test actually calls a builtin or syntax added for this wave
# (replace <FEATURE_BUILTIN> with the builtin you added)
if ! grep -q '<FEATURE_BUILTIN>' tests/gold_standard/ipc/<your_test>.vuma; then
  echo "FAIL: test does not call the feature's builtin"
  exit 1
fi

# 3. The build is green
cargo build --workspace 2>&1 | grep -E '^error' && echo "FAIL: build broken" && exit 1

# 4. The test compiles and runs to the expected exit code
./target/debug/compile_dump tests/gold_standard/ipc/<your_test>.vuma /tmp/test.bin x86_64 || echo "FAIL: compile failed"
/tmp/test.bin; rc=$?
[ "$rc" = "<EXPECTED>" ] || echo "FAIL: exit $rc, expected <EXPECTED>"
```

---

## 10. FINAL WARNING

Two prior agents have failed this task. The first wrote library code and didn't wire it. The second wrote fake tests and claimed success. **You will be caught** if you repeat these patterns — the acceptance criteria in §6 check for emitted code and parser constructs, not symbol mentions. A `use` import does not pass. A comment does not pass. A renamed `simple_send.vuma` does not pass.

The only way to pass is to actually implement each feature in the file the spec names, create a test that exercises it, and verify the test runs to the documented exit code.

**Go. Start with Phase 0 (delete the 6 fake tests), then Phase A (Wave 8b).**
