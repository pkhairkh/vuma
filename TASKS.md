# VUMA Remediation Task Plan

> **Source:** Derived from a full source-code audit (docs ignored). Every task
> carries a file:line reference so it can be verified independently.
> **Wave principle:** tasks inside a single wave touch **disjoint files/domains**
> and can be executed in parallel. Waves are ordered so that later waves depend
> only on earlier ones.
> **Status key:** `[ ]` pending · `[~]` in progress · `[x]` done · `[!]` blocked

---

## Domain Legend

| Tag        | Domain                                                        |
| ---------- | ------------------------------------------------------------ |
| `BE-*`     | A specific codegen backend (x86_64, aarch64, riscv64, …)     |
| `IR`       | The IR layer (`src/codegen/src/ir.rs`)                       |
| `SCG`      | Semantic Computation Graph (`src/scg/`)                      |
| `IVE`      | Inference & Verification Engine (`src/ive/`)                 |
| `BD`       | Bidirectional inference (`src/bd/`)                          |
| `PROOF`    | Proof system (`src/proof/`)                                  |
| `OPT`      | IR-level optimizer (`src/codegen/src/opt.rs` + siblings)    |
| `SCHED`    | Instruction scheduler (`src/codegen/src/scheduler.rs`)      |
| `REGALLOC` | Register allocator (`src/codegen/src/regalloc.rs`)          |
| `EGRAPH`   | E-graph optimizer (`src/codegen/src/egraph.rs`)             |
| `LOWER`    | Lowering passes (`monomorphize`, `closures`, `control_flow`)|
| `PIPE`     | Pipeline integration (`src/pipeline.rs`, `src/api.rs`)      |
| `MEMSAFE`  | Memory safety analyzer (`src/codegen/src/memory_safety.rs`) |
| `COR`      | Continuous Optimization Runtime (`src/cor/`)                |
| `STD`      | Standard library (`src/std/`)                                |
| `FFI`      | FFI layer (`src/ffi.rs`)                                     |
| `BOOT`     | Bootstrap / self-hosting (`src/bootstrap/`, `womb/lang/`)    |
| `DEP`      | Dependency hygiene (`Cargo.toml`)                            |
| `TEST`     | Test infrastructure (`src/tests/`)                           |

---

# Wave 1 — Critical one-line bug fixes & dead-dependency removal

> All tasks touch disjoint files. Zero dependencies between them.

- [x] **[BE-sparc64]** Verify `setsockopt=355` is correct against `arch/sparc/include/uapi/asm/unistd.h` (audit flagged it as a bug but commit `899a287` claims correction; confirm and close). `src/codegen/src/sparc64.rs:4054` — **VERIFIED NOT-A-BUG**: 355 is the correct modern sparc64 `__NR_setsockopt`. Added clarifying comment to prevent re-flagging.
- [x] **[BE-alpha]** Fix `pipe` stub to propagate failure instead of unconditionally returning 0. `src/codegen/src/alpha.rs:1638` — replaced the confused hand-encoding attempt + unconditional `OR ZERO,ZERO,R0` with the existing `Instruction::Cmovne { ra: R19, rb: R1, rc: R0 }` (opcode 0x11, function 0x26) so `callsys` errors propagate as `-1`.
- [x] **[BE-riscv32]** Switch `clock_gettime` from legacy `113` to `clock_gettime64=403` (Y2038). `src/codegen/src/riscv32.rs:6486`
- [x] **[DEP]** Remove dead workspace deps `colored`, `anyhow`, `env_logger` (declared, never `use`d). `Cargo.toml:54-57` + `src/tests/Cargo.toml:23` — removed from both `[workspace.dependencies]`, root `[dependencies]`, and `src/tests/Cargo.toml`.
- [x] **[BE-hppa]** Replace no-op `__vuma_free` with real `munmap` syscall stub. `src/codegen/src/hppa.rs:1488` — the real munmap stub already existed at `hppa.rs:2129` but the Call handler was short-circuiting `__vuma_free` calls with NOPs. Removed the skip-code so the real stub is now invoked.
- [x] **[BE-m68k]** Replace no-op `__vuma_free` with real `munmap` syscall stub. `src/codegen/src/m68k.rs:1799` — replaced `RTS`-only no-op with `D2=4096; D0=91; TRAP #0; RTS` (real `__NR_munmap=91`).
- [x] **[BE-sparc64]** Replace no-op `__vuma_free` with real `munmap` syscall stub. `src/codegen/src/sparc64.rs:3965` — replaced `JMPL %o7+8`-only no-op with `%o1=4096; OR %g0,73,%g1; TA 0x6d; JMPL %o7+8; NOP` (real `__NR_munmap=73`).

---

# Wave 2 — Restore `print_int` / `print_hex` on 7 silent backends

> One task per backend. Each touches only that backend's stub table.

- [x] **[BE-aarch64]** Restore `print_int`/`print_hex` stubs (currently commented out as no-op). `src/codegen/src/backend.rs:2724-2728` — registered bare-name `print_int`/`print_hex` aliases pointing at the existing `__vuma_print_*` runtime offsets (`rt_int_off`/`rt_hex_off`). The runtime prologue/epilogue already saves/restores every caller-saved register it touches (X1, X2, X3, X8, X9, X10), so the previous "clobbers locals" concern is moot.
- [x] **[BE-s390x]** Restore `print_int`/`print_hex` stubs. `src/codegen/src/s390x.rs:2656,2750` — uncommented the `syscall_stubs.push(("print_int", code))` and `("print_hex", code))` lines; the full decimal/hex conversion stub code was already present and well-formed. The `__vuma_print_int`/`__vuma_print_hex` aliases (line 2787-2788) now correctly resolve to the same offsets.
- [x] **[BE-alpha]** Restore `print_int`/`print_hex` stubs. `src/codegen/src/alpha.rs:1776,1837` — uncommented both `syscall_stubs.push` lines; stub code uses proper `patch_alpha_branch` helpers and balanced `SP -= 32`/`SP += 32; RET` frame.
- [x] **[BE-hppa]** Restore `print_int`/`print_hex` stubs. `src/codegen/src/hppa.rs:2077,2116` — uncommented both pushes; stubs use balanced `R30 -= 48`/`R30 += 48; BV R2(R0)` frames.
- [x] **[BE-m68k]** Restore `print_int`/`print_hex` stubs. `src/codegen/src/m68k.rs:2171,2256` — uncommented both pushes; stubs use `LINK A6, #-N`/`UNLK A6; RTS` frames and save/restore D3-D7 via MOVEM. The `__vuma_print_*` aliases (line 2313-2314) now correctly resolve.
- [x] **[BE-riscv32]** Register bare `print_int`/`print_hex` (currently only `__vuma_*`). `src/codegen/src/riscv32.rs` — refactored `build_riscv32_runtime` to return `(Vec<u8>, hex_off, int_off, newline_off)` and registered both `__vuma_*` and bare `print_int`/`print_hex` at their correct per-function offsets (fixing a dormant bug where all three symbols previously aliased to offset 0).
- [x] **[BE-riscv64]** Register bare `print_int`/`print_hex`. `src/codegen/src/riscv64.rs:6431` — same refactor as riscv32: `build_riscv64_runtime` now returns offsets, and bare `print_int`/`print_hex` are registered at the correct `rt_int_off`/`rt_hex_off` within the runtime blob.

---

# Wave 3 — Restore `print_newline` consistency on all backends

> One task per backend. Independent stub additions.

- [x] **[BE-x86_32]** Add `print_newline` stub. `src/codegen/src/x86_32/mod.rs` — added `write(1,&newline,1)` stub with EBX save/restore + `__vuma_print_newline` alias registration.
- [x] **[BE-alpha]** Add `print_newline` stub. `src/codegen/src/alpha.rs` — added `LDA SP,-16; STB '\n',0(SP); R16=1,R17=SP,R18=1,R0=4; CALL_PAL 0x83; LDA SP,16; RET` stub.
- [x] **[BE-hppa]** Add `print_newline` stub. `src/codegen/src/hppa.rs` — added `LDO -16(SP); STB '\n',0(SP); R26=1,R25=SP,R24=1,R20=4; GATE; LDO 16(SP); BV R2(R0)` stub.
- [x] **[BE-m68k]** Add `print_newline` stub. `src/codegen/src/m68k.rs` — added `MOVEQ #10,D0; MOVE.L D0,-(SP); D1=1,D2=SP,D3=1,D0=4; TRAP #0; ADDQ.L #4,SP; RTS` stub + `__vuma_print_newline` alias.
- [x] **[BE-s390x]** Add `print_newline` stub. `src/codegen/src/s390x.rs` — added `SP-=16; STC '\n',0(SP); R2=1,R3=SP,R4=1,R1=4; SVC 0; SP+=16; BR R14` stub + `__vuma_print_newline` alias.
- [x] **[BE-riscv32]** Add `print_newline` stub. `src/codegen/src/riscv32.rs` — runtime blob already had `__vuma_print_newline`; added bare `print_newline` alias to `func_offsets`.
- [x] **[BE-riscv64]** Add `print_newline` stub. `src/codegen/src/riscv64.rs` — runtime blob already had `__vuma_print_newline`; added bare `print_newline` alias to `func_offsets`.

---

# Wave 4 — wasm32 WASI coverage: filesystem ops

> Each task maps a POSIX name to a `vuma.*` host function import (not raw WASI,
> because WASI preview1's filesystem APIs are capability-based with 8-arg
> signatures incompatible with POSIX). All in `src/codegen/src/wasm32/mod.rs`
> and `scripts/wasm32_runner.py`.

- [x] **[BE-wasm32]** Map `open` → `vuma.open(path, flags, mode)` host fn (idx 18). Calls real `os.open()`.
- [x] **[BE-wasm32]** Map `stat`/`lstat`/`fstat` → `vuma.stat`/`vuma.lstat`/`vuma.fstat` host fns (idx 19-21). Calls real `os.stat()`/`os.lstat()`/`os.fstat()`, writes simplified 64-byte struct stat to buffer.
- [x] **[BE-wasm32]** Map `unlink` → `vuma.unlink(path)` host fn (idx 22). Calls real `os.unlink()`.
- [x] **[BE-wasm32]** Map `mkdir` → `vuma.mkdir(path, mode)` host fn (idx 23). Calls real `os.mkdir()`.
- [x] **[BE-wasm32]** Map `rename` → `vuma.rename(oldpath, newpath)` host fn (idx 25). Calls real `os.rename()`.
- [x] **[BE-wasm32]** Map `rmdir` → `vuma.rmdir(path)` host fn (idx 24). Calls real `os.rmdir()`.
- [x] **[BE-wasm32]** Map `link`/`symlink`/`readlink` → `vuma.link`/`vuma.symlink`/`vuma.readlink` host fns (idx 26-28). Calls real `os.link()`/`os.symlink()`/`os.readlink()`.
- [x] **[BE-wasm32]** Fix stale VUMA_*_IDX constants: old VUMA_STAT_IDX=15/FSTAT=16/LSTAT=17 conflicted with actual read/write/close imports at indices 15-17. Updated to correct indices 19-21. Old VUMA_OPEN_IDX=53 updated to 18.

---

# Wave 5 — wasm32 WASI coverage: sockets, process, sync

> Currently silently return -1. Either map to host functions or document as
> unsupported with a real error code (not silent -1).

- [x] **[BE-wasm32]** Resolve `socket`/`bind`/`listen`/`accept`/`connect` — either add `vuma.socket.*` host imports or return `-ENOSYS`. `src/codegen/src/wasm32/mod.rs` + `scripts/wasm32_runner.py` — added `vuma.socket`/`bind`/`listen`/`accept`/`connect` host imports (indices 29-33) backed by the real OS socket layer via Python's `socket` module (fd wrapped with `fileno=` + `detach()` so GC doesn't close it). sockaddr_in (AF_INET) marshaled to/from wasm linear memory.
- [x] **[BE-wasm32]** Resolve `send`/`recv`/`sendto`/`recvfrom`/`sendmsg`/`recvmsg`. — added `vuma.send`/`recv`/`sendto`/`recvfrom` host imports (indices 34-37). `sendmsg`/`recvmsg` intentionally NOT mapped: they resolve to the generic `-ENOSYS` stub (msghdr struct marshaling — iovec arrays + ancillary data — is too complex for the wasm32 bridge; documented in the constants comment).
- [x] **[BE-wasm32]** Resolve `setsockopt`/`getsockopt`/`shutdown`. — added `vuma.setsockopt`/`getsockopt`/`shutdown` host imports (indices 38-40). optval marshaled as bytes (4-byte opts unpacked as i32); getsockopt writes the value + length back to the caller's buffers.
- [x] **[BE-wasm32]** Resolve `mmap`/`munmap`/`mprotect` (no-op or document). — added `vuma.mmap`/`munmap`/`mprotect` host imports (indices 41-43). `mmap` anonymous (`MAP_ANONYMOUS=0x20`) bump-allocates in wasm linear memory (region base 1 MiB, grows memory via `memory.grow` as needed); file-backed `mmap` returns `MAP_FAILED` (-1, documented unsupported). `munmap`/`mprotect` are no-ops returning 0 (wasm has no page protection / the bump allocator can't free).
- [x] **[BE-wasm32]** Resolve `futex`/`clone` (or document as unsupported). — intentionally NOT mapped: `futex` (inter-thread locking) and `clone` (thread/process creation) are fundamentally incompatible with wasm32's single-threaded sandbox. They resolve to the generic `-ENOSYS` stub so callers detect they are unsupported (documented in the constants comment).
- [x] **[BE-wasm32]** Resolve `nanosleep`/`clock_gettime` (already have `clock_time_get` — alias). — `clock_gettime` was already aliased to WASI `clock_time_get` (index 5) in `func_name_to_idx`; left as-is. Added `vuma.nanosleep` host import (index 44) backed by real `time.sleep`: reads `struct timespec { tv_sec, tv_nsec }` (16 bytes, i64+i64 LE) from `req_ptr`, sleeps, writes zero remainder to `rem_ptr` if non-NULL.
- [x] **[BE-wasm32]** Replace silent -1 fallback with explicit `-ENOSYS` so callers can detect unsupported syscalls. `src/codegen/src/wasm32/mod.rs` — the generic unknown-extern stub now writes `-ENOSYS` (-38) to linear memory address 0 (the codegen's extern-return slot) AND returns `-ENOSYS` on the wasm stack, replacing the previous `i32.const -1` that was indistinguishable from a real runtime error. Added `ENOSYS_ERRNO=38` constant. The `() -> i32` stub correctly serves callers of any arity because wasm `call` only pops `params.len()` (=0) values, leaving extra caller args as stack orphans that the codegen's block-end Drop-all logic cleans up.

---

# Wave 6 — `mmap` ABI normalization on 32-bit backends

> Each backend's `mmap` extern needs the same offset-unit handling as
> `__vuma_alloc`. Independent per backend.

- [x] **[BE-x86_32]** Expose `mmap2` semantics for bare `extern "C" { fn mmap(...) }`. `src/codegen/src/x86_32/mod.rs:2139` — VERIFIED ALREADY-CORRECT: the bare `mmap` stub (`x86_32/mod.rs:2336`) reads the caller's byte offset from `[ESP+24]`, shifts it `>> 12` to pages, places it in EBP (i386 syscall arg6), and invokes `sys_mmap2=192` — i.e. the same mmap2/page-granular offset path as `__vuma_alloc` (line ~2139) but accepting the caller's offset instead of hardcoding 0. Added a wave-6 documentation block linking the two stubs and restating the i386 syscall ABI.
- [x] **[BE-m68k]** Same — `mmap2=192` offset-in-pages semantics. `src/codegen/src/m68k.rs:1831` — FIXED: the bare `mmap` was a `simple_stub(192)` that neither pushed a 6th syscall arg nor converted the offset (and the inline comment wrongly claimed m68k passes 6 args in D1-D6 — m68k syscalls use D1-D5 + stack). Replaced with a dedicated mmap2 stub that pushes `pgoff=0` (6th syscall arg), calls `sys_mmap2=192`, then pops with `ADDQ.L #4, A7` (0x58CF) — matching `__vuma_alloc`'s mmap2/offset-in-pages/offset=0 path exactly. The m68k VUMA calling convention only passes 5 args (D1-D5; `Gpr::arg_register(5)` is `None`), so a byte offset cannot be passed; the stub therefore hardcodes `pgoff=0` (anonymous-only), identical to `__vuma_alloc`. File-backed mmap with a non-zero offset is documented as unsupported on m68k (would require extending the Call lowering — out of wave-6 scope). BONUS FIX: `__vuma_alloc`'s "pop" bytes were `0x5F 0xC4` which decode to `SLE D4` (a no-op on SP), not `ADDQ.L #4, SP`; this latent bug would crash `__vuma_alloc` if ever called (RTS pops pgoff as retaddr). Corrected to `0x58 0xCF` (real `ADDQ.L #4, A7`).
- [x] **[BE-arm32]** Verify `mmap=90` struct-pointer-arg path; document in stub. `src/codegen/src/arm32/mod.rs:7407-7413` — VERIFIED ALREADY-CORRECT: ARM EABI's `sys_old_mmap=90` takes a single struct-pointer in R0; the stub builds `mmap_arg_struct{addr,len,prot,flags,fd,offset}` on the stack (loading fd/offset from the caller's AAPCS stack args), sets R0=SP, R7=90, SVC #0. The struct's `offset` field is in BYTES — same offset unit as `__vuma_alloc` (which uses the same struct-pointer path with offset=0). Added a wave-6 documentation block above the stub.
- [x] **[BE-ppc64]** Verify legacy `mmap=90` path; document in stub. `src/codegen/src/ppc64/mod.rs:6268` — VERIFIED: ppc64's `sys_mmap=90` is the DIRECT 6-arg form (addr,len,prot,flags,fd,offset) in R3-R8 with `offset` in BYTES (no mmap2 on ppc64). The `simple_stub(90)` passes caller's R3-R8 straight through — identical to `__vuma_alloc` (which sets R3-R8 then `LI R0,90; SC`). Same offset unit (bytes/R8). ppc64 CC passes 8 args (R3-R10), so all 6 mmap args fit. Added wave-6 documentation comment.
- [x] **[BE-alpha]** Verify `mmap=113` path; document in stub. `src/codegen/src/alpha.rs:1536` — VERIFIED: alpha's `__NR_mmap=113` is the DIRECT 6-arg form in R16-R21 with `offset` in BYTES (no mmap2; generic sys_mmap converts in-kernel). The `simple_stub(113)` passes caller's R16-R21 straight through — same as `__vuma_alloc` (R21=0, R0=113, CALL_PAL 0x83). Same offset unit (bytes/R21). CLEANED UP the stale comment block that incorrectly stated `__NR_mmap=90` (90 is `osf_old_mmap`/dup2 on alpha, NOT the modern mmap). Added wave-6 documentation.
- [x] **[BE-riscv32]** Verify 6-arg `mmap=222` on RV32 (no `mmap2`). — VERIFIED: RV32's `sys_mmap=222` is the DIRECT 6-arg form in a0-a5 with `offset` in BYTES (rv32 has NO mmap2; generic sys_mmap converts in-kernel). The `simple_stub(222)` passes caller's a0-a5 straight through — same as `__vuma_alloc` (a5=0, a7=222, ECALL). Same offset unit (bytes/a5). RV32 CC passes 8 args (a0-a7), so all 6 mmap args fit. Added wave-6 documentation comment at `riscv32.rs:6370`.
- [x] **[BE-x86_64]** Verify `mmap=9` direct 6-arg path; document ABI. — VERIFIED: x86_64's `sys_mmap=9` is the DIRECT 6-arg form (RDI,RSI,RDX,R10,R8,R9) with `offset` in BYTES (no mmap2; sys_mmap converts in-kernel). The stub moves arg4 flags from SysV RCX → syscall-ABI R10 and leaves the other 5 args in place — same offset unit as `__vuma_alloc` (R9=0, RAX=9, syscall). x86_64 CC passes 6 args (RDI/RSI/RDX/RCX/R8/R9), so all 6 mmap args fit. Added wave-6 documentation block at `x86_64/mod.rs:2884`.

---

# Wave 7 — Missing POSIX syscalls: file metadata ops (all backends)

> One task per syscall family; each task touches every backend's stub table
> but the families are independent. Run all 7 in parallel.

- [ ] **[BE-all]** Add `mkdir`/`rmdir`/`rename`/`link`/`symlink`/`readlink` to every backend's `syscall_stubs`.
- [ ] **[BE-all]** Add `chmod`/`chown`/`umask`/`fchmod`/`fchown`.
- [ ] **[BE-all]** Add `*at` variants: `openat`/`unlinkat`/`renameat`/`linkat`/`symlinkat`/`readlinkat`/`faccessat`/`fchmodat`/`fchownat`.
- [ ] **[BE-all]** Add `ftruncate`/`fsync`/`fdatasync`/`sync`/`syncfs`.
- [ ] **[BE-all]** Add `pread`/`pwrite`/`readv`/`writev`/`preadv`/`pwritev`.
- [ ] **[BE-all]** Add `lseek` (where missing) / `dup`/`dup2`/`dup3`/`fcntl`/`ioctl`.
- [ ] **[BE-all]** Add `getcwd`/`chdir`/`fchdir`/`chroot`.

---

# Wave 8 — Missing POSIX syscalls: process & identity (all backends)

- [ ] **[BE-all]** Add `getuid`/`geteuid`/`getgid`/`getegid`/`setuid`/`setgid`/`setresuid`/`setresgid`.
- [ ] **[BE-all]** Add `getpid`/`getppid`/`getsid`/`setsid`/`setpgid`/`getpgid`/`getpgrp`.
- [ ] **[BE-all]** Add `vfork`/`clone`/`clone3`/`waitid`/`wait4`.
- [ ] **[BE-all]** Add `execve`/`execveat`/`exit_group` (where missing).
- [ ] **[BE-all]** Add `kill`/`tgkill`/`tkill`/`rt_sigaction`/`rt_sigprocmask`/`rt_sigreturn`.
- [ ] **[BE-all]** Add `getdents64`/`readdir`/`getdents`.
- [ ] **[BE-all]** Add `prctl`/`arch_prctl` (x86_64 only) /`uname`/`sysinfo`.

---

# Wave 9 — Missing POSIX syscalls: system & advanced (all backends)

- [ ] **[BE-all]** Add `mlock`/`munlock`/`mlockall`/`munlockall`/`mincore`/`madvise`.
- [ ] **[BE-all]** Add `getrlimit`/`setrlimit`/`prlimit64`/`getrusage`/`times`/`umask`.
- [ ] **[BE-all]** Add `getrandom` (replace silent fallback on non-wasm).
- [ ] **[BE-all]** Add `eventfd`/`timerfd_create`/`timerfd_settime`/`timerfd_gettime`/`signalfd`.
- [ ] **[BE-all]** Add `epoll_create1`/`epoll_ctl`/`epoll_wait` (verify sparc64 numbers).
- [ ] **[BE-all]** Add `inotify_init1`/`inotify_add_watch`/`inotify_rm_watch`.
- [ ] **[BE-all]** Add `ptrace`/`madvise`/`msync`/`mremap`/`munlock`.

---

# Wave 10 — Introduce `IRInstr::Syscall` (IR + parser)

> Currently syscalls go through `IRInstr::Call { is_extern: true }` resolved by
> name. Introduce a first-class syscall IR node.

- [ ] **[IR]** Add `IRInstr::Syscall { nr: u32, args: Vec<IRValue>, dst: Option<IRValue> }` variant. `src/codegen/src/ir.rs:1211-1533`
- [ ] **[SCG]** Add `NodePayload::Syscall` / `EdgeKind::SyscallArg` so SCG tracks syscalls as first-class. `src/scg/src/node.rs`, `src/scg/src/edge.rs`
- [ ] **[PARSER]** Parse `syscall(nr, args...)` syntax in `.vuma`. `src/parser/src/parser.rs`
- [ ] **[PARSER]** Lower `syscall(...)` AST node → `IRInstr::Syscall` in `to_scg.rs`. `src/parser/src/to_scg.rs`
- [ ] **[IR]** Add `IRInstr::Syscall` size/effect metadata (may-read/may-write/aborts).
- [ ] **[PIPE]** Add a verification-level "syscall allowlist" so unknown syscalls become compile errors instead of silent FFI return-0.

---

# Wave 11 — Backend `IRInstr::Syscall` emission: tier-1 backends

> Each backend implements `encode_syscall_instr` independently.

- [ ] **[BE-x86_64]** Emit `mov eax, nr; syscall` from `IRInstr::Syscall`. `src/codegen/src/x86_64/mod.rs`
- [ ] **[BE-aarch64]** Emit `MOV X8, nr; SVC #0`. `src/codegen/src/backend.rs`
- [ ] **[BE-riscv64]** Emit `LI a7, nr; ECALL`. `src/codegen/src/riscv64.rs`
- [ ] **[BE-riscv32]** Emit `LI a7, nr; ECALL`. `src/codegen/src/riscv32.rs`
- [ ] **[BE-arm32]** Emit `MOV r7, nr; SVC #0`. `src/codegen/src/arm32/mod.rs`
- [ ] **[BE-x86_32]** Emit `MOV eax, nr; INT 0x80`. `src/codegen/src/x86_32/mod.rs`

---

# Wave 12 — Backend `IRInstr::Syscall` emission: tier-2/3 backends

- [ ] **[BE-loongarch64]** Emit `addi.d $a7, $r0, nr; SYSCALL 0x0`. `src/codegen/src/loongarch64/mod.rs`
- [ ] **[BE-mips64]** Emit `LI V0, nr; SYSCALL`. `src/codegen/src/mips64/mod.rs`
- [ ] **[BE-ppc64]** Emit `LI R0, nr; SC`. `src/codegen/src/ppc64/mod.rs`
- [ ] **[BE-s390x]** Emit `LGFI R1, nr; SVC 0`. `src/codegen/src/s390x.rs`
- [ ] **[BE-sparc64]** Emit `OR %g0, nr, %g1; ta 0x6d`. `src/codegen/src/sparc64.rs`
- [ ] **[BE-alpha]** Emit `LDI R0, nr; CALL_PAL 0x83`. `src/codegen/src/alpha.rs`
- [ ] **[BE-hppa]** Emit `LDI R20, nr; GATE`. `src/codegen/src/hppa.rs`
- [ ] **[BE-m68k]** Emit `MOVEQ #nr, D0; TRAP #0`. `src/codegen/src/m68k.rs`
- [ ] **[BE-wasm32]** Map `IRInstr::Syscall` to WASI imports or `vuma.*` host fns by number. `src/codegen/src/wasm32/mod.rs`

---

# Wave 13 — Big-endian wrapper backends inherit `IRInstr::Syscall`

> Wrapper backends should automatically inherit from their parent. Verify and
> document.

- [ ] **[BE-aarch64_be]** Verify `IRInstr::Syscall` emission inherited from aarch64. `src/codegen/src/aarch64_be.rs:13-23`
- [ ] **[BE-armeb]** Verify inheritance from arm32 (BE32 word-swap). `src/codegen/src/armeb.rs:12-22`
- [ ] **[BE-mips64be]** Verify inheritance from mips64. `src/codegen/src/mips64be.rs:14-26`
- [ ] **[BE-ppc64le]** Verify inheritance from ppc64 (ELFv2). `src/codegen/src/ppc64le.rs:65-72`
- [ ] **[BE-all]** Add a cross-backend conformance test asserting every backend emits a non-empty syscall instruction for `IRInstr::Syscall { nr: 1, ... }`.

---

# Wave 14 — Delete dead parallel IVE code: `vuma/src/invariant_*`

> 5,880 LOC of orphaned MSG-based verifiers never called outside their own
> `#[cfg(test)]`. Confirm zero production callers, then delete.

- [ ] **[IVE]** Verify zero production callers of `check_liveness`/`check_exclusivity`/`check_origin`/`check_interpretation`/`check_cleanup` in `src/vuma/src/`.
- [ ] **[IVE-DEL]** Delete `src/vuma/src/invariant_liveness.rs` (1,105 LOC).
- [ ] **[IVE-DEL]** Delete `src/vuma/src/invariant_exclusivity.rs` (1,101 LOC).
- [ ] **[IVE-DEL]** Delete `src/vuma/src/invariant_origin.rs` (905 LOC).
- [ ] **[IVE-DEL]** Delete `src/vuma/src/invariant_interpretation.rs` (1,632 LOC).
- [ ] **[IVE-DEL]** Delete `src/vuma/src/invariant_cleanup.rs` (1,137 LOC).
- [ ] **[IVE-DEL]** Remove module declarations from `src/vuma/src/lib.rs:55-59`. Audit MSG (`src/vuma/src/msg.rs`) — if only the deleted invariant_* used it, delete MSG too.

---

# Wave 15 — Delete dead parallel IVE code: `ive/src/bd_solver.rs` + hardened family

> 1,521 LOC parallel BD solver + `verify_all_hardened` family never invoked.

- [ ] **[IVE]** Verify zero production callers of `BDConstraintSolver` in `src/ive/src/bd_solver.rs`.
- [ ] **[IVE-DEL]** Delete `src/ive/src/bd_solver.rs` (1,521 LOC).
- [ ] **[IVE-DEL]** Delete `verify_all_hardened` + `check_capability_flow` + `check_aliasing_integrity` + `validate_derivation_chain` from `src/ive/src/verification.rs`.
- [ ] **[IVE-DEL]** Delete `compute_path_sensitive_liveness` from `src/ive/src/liveness.rs:1430` (unused in production).
- [ ] **[IVE]** Update `src/ive/src/lib.rs` re-exports.
- [ ] **[TEST]** Remove or migrate tests that referenced the deleted code.

---

# Wave 16 — Wire IVE interprocedural, modular, and constant-time analyses

> Real implementations (901 + 410 + 167 LOC) currently never invoked. Wire
> them in as opt-in verification levels.

- [ ] **[IVE-WIRE]** Wire `interprocedural::compute_summaries` + `verify_interprocedural_invariants` into `InvariantAggregator` at `Exhaustive` level. `src/ive/src/interprocedural.rs:121,324`
- [ ] **[IVE-WIRE]** Wire `modular::verify_all_functions` as a new `VerificationLevel::Modular`. `src/ive/src/modular.rs`
- [ ] **[IVE-WIRE]** Fix `modular.rs:84-86` "mark all allocations as escaping" — implement real escape analysis instead of stub.
- [ ] **[IVE-WIRE]** Wire `constant_time::verify_constant_time` as a 6th invariant under `VerificationLevel::ConstantTime`. `src/ive/src/constant_time.rs:52`
- [ ] **[IVE-WIRE]** Add `VerificationLevel::Hardened` that runs all 6 invariants + interprocedural + modular.
- [ ] **[TEST]** Add end-to-end tests for each new level.

---

# Wave 17 — Proof system: implement missing tactics & fix stubs

- [ ] **[PROOF]** Implement `prove_interpretation` tactic in `src/proof/src/interpretation_proofs.rs` (currently 36 LOC of data structures only).
- [ ] **[PROOF]** Fix `WellFoundedOrdering::is_well_founded` hardcoded `true` → real check (finite region set: verify all referenced regions have assigned ranks). `src/proof/src/liveness_proofs.rs:141-143`
- [ ] **[PROOF]** Replace string-matching in `ProofBundle::verify_cross_invariant_consistency` with structural `Judgment` matching. `src/proof/src/composition.rs:114`
- [ ] **[PROOF]** Add `Judgment::InterpretationCompatible` and rules for the new `prove_interpretation` tactic.
- [ ] **[TEST]** Add proof-system tests covering each new tactic.

---

# Wave 18 — Proof system: wire into IVE pipeline

> Currently `build_proof_bundle` returns `ProofBundle::new()` (empty). Wire it
> for real.

- [ ] **[PROOF-WIRE]** Implement `build_proof_bundle` to extract `ProofSCG`/`ProofMSG` and call `prove_*` tactics. `src/api.rs:1431-1442`
- [ ] **[PROOF-WIRE]** Call `ProofChecker::check` on each generated proof in `InvariantAggregator::run_single_check` at `Exhaustive` level.
- [ ] **[PROOF-WIRE]** Only attach `Evidence::FormalProof` when `ProofChecker` returns `CheckResult::Valid`. `src/ive/src/invariant_aggregator.rs:676-685`
- [ ] **[PROOF-WIRE]** Remove the fake `ProofStep::from(format!("proof of {} verified by IVE", …))` string-evidence. `src/ive/src/invariant_aggregator.rs:676-685`
- [ ] **[PROOF-WIRE]** Make `api.rs:540-552` cross-check loop upgrade `Unverified → Fail` when proof status is `Failed`.
- [ ] **[TEST]** Add end-to-end test: a verified program produces a non-empty `ProofBundle` with `all_proven() == true`.

---

# Wave 19 — Close verification escape hatches

- [ ] **[IVE]** Remove user-default `VerificationLevel::None`; require explicit `--no-verify` flag. `src/pipeline.rs:4846-4847`
- [ ] **[IVE]** Add `--strict-verification` flag making `OverallVerdict::Inconclusive` block compilation. `src/pipeline.rs:4861-4864`
- [ ] **[IVE]** Change `Quick` mode to run all 5 invariants at reduced depth (instead of skipping liveness/interpretation/cleanup).
- [ ] **[IVE]** Fix cleanup-extractor false positive for top-level `region` declarations flagged as leaks. `src/api.rs:1466-1474`
- [ ] **[IVE]** Make IVE `max_paths` (64) and `max_path_length` (256) configurable via `CompileConfig`. `src/ive/src/liveness.rs:839`, `src/ive/src/cleanup.rs:721-727`
- [ ] **[TEST]** Add regression tests for each escape hatch.

---

# Wave 20 — Memory safety as a blocking pass

> `CompileConfig.memory_safety: bool` is set but never read. Wire it.

- [ ] **[MEMSAFE]** Read `CompileConfig.memory_safety` in pipeline and gate the analyzer. `src/pipeline.rs:178,234`
- [ ] **[MEMSAFE-WIRE]** Run `MemorySafetyAnalyzer::analyze` as a blocking pass at the IVE stage. `src/codegen/src/memory_safety.rs:442`
- [ ] **[MEMSAFE-WIRE]** Use `analyze_with_scg_liveness` (the SCG-liveness variant) for use-after-free / uninit-read detection. `src/codegen/src/memory_safety.rs:960`
- [ ] **[MEMSAFE]** Add `--no-memory-safety` escape hatch (with compile-time warning).
- [ ] **[MEMSAFE]** Fix the top-level `region` false-positive leak (`src/api.rs:1466-1474`) so the analyzer doesn't flag every program.
- [ ] **[TEST]** Add regression test: a UAF program is rejected at compile time.

---

# Wave 21 — Real register allocation: emit-path plumbing

> Currently `LinearScanAllocator` runs and its output is discarded; emit uses
> stack-slot lowering for every vreg.

- [ ] **[REGALLOC]** Make `emit_binary` accept `&[AllocationResult]` and consult it. `src/codegen/src/emit.rs:4775`
- [ ] **[REGALLOC]** Remove the `STACK_SLOT_VREG_THRESHOLD = 0` hack that forces every function through `emit_function_stack_slot`. `src/codegen/src/emit.rs:78`
- [ ] **[REGALLOC]** Add spill-slot emission for evicted vregs in `emit_function_regalloc`.
- [ ] **[REGALLOC]** Add move/coalescing emission across register classes.
- [ ] **[REGALLOC]** Wire `DebugInfo::regalloc_results` (currently write-only) into emit. `src/pipeline.rs:5058`
- [ ] **[TEST]** Add `regalloc_correctness` test running SHA256d end-to-end on x86_64 with real regalloc.

---

# Wave 22 — Real register allocation: tier-1 backends

- [ ] **[BE-x86_64]** Implement `emit_function_regalloc` consuming `AllocationResult` (RAX/RCX/RDX/RSI/RDI/R8-R11 + spills). `src/codegen/src/x86_64/mod.rs`
- [ ] **[BE-aarch64]** Same (X0-X28 + spills). `src/codegen/src/backend.rs`
- [ ] **[BE-riscv64]** Same (a0-a7, t0-t6, s0-s11 + spills). `src/codegen/src/riscv64.rs`
- [ ] **[BE-arm32]** Same (r0-r3, r4-r10 + spills). `src/codegen/src/arm32/mod.rs`
- [ ] **[BE-loongarch64]** Same (a0-a7, t0-t8, s0-s9 + spills). `src/codegen/src/loongarch64/mod.rs`

---

# Wave 23 — Real register allocation: tier-2/3 backends

- [ ] **[BE-mips64]** `emit_function_regalloc` (v0-v1, a0-a3, t0-t9, s0-s7 + spills). `src/codegen/src/mips64/mod.rs`
- [ ] **[BE-ppc64]** `emit_function_regalloc` (r3-r10, r14-r31 + spills). `src/codegen/src/ppc64/mod.rs`
- [ ] **[BE-s390x]** `emit_function_regalloc` (r2-r6, r7-r15 + spills). `src/codegen/src/s390x.rs`
- [ ] **[BE-sparc64]** `emit_function_regalloc` (o0-o5, l0-l7, i0-i5 + spills). `src/codegen/src/sparc64.rs`
- [ ] **[BE-alpha]** `emit_function_regalloc` (a0-a5, t0-t9, s0-s6 + spills). `src/codegen/src/alpha.rs`
- [ ] **[BE-hppa]** `emit_function_regalloc` (arg0-3, r1-r18, r26-r31 + spills). `src/codegen/src/hppa.rs`
- [ ] **[BE-m68k]** `emit_function_regalloc` (d0-d7, a0-a5 + spills). `src/codegen/src/m68k.rs`
- [ ] **[BE-x86_32]** `emit_function_regalloc` (eax/edx/ecx/ebx/esi/edi + spills). `src/codegen/src/x86_32/mod.rs`

---

# Wave 24 — Register allocator: dead-code deletion & `TargetAgnosticRegAlloc`

- [ ] **[REGALLOC]** Decide: delete `regalloc.rs::RegAllocator` (legacy greedy, gated out by `STACK_SLOT_VREG_THRESHOLD=0`) or refactor.
- [ ] **[REGALLOC-WIRE]** Wire `TargetAgnosticRegAlloc::allocate_function` (currently never called) for backends without a custom allocator. `src/codegen/src/regalloc.rs:1980`
- [ ] **[REGALLOC]** Add per-backend `RegisterClass` + `TargetDesc` modeling for the target-agnostic allocator.
- [ ] **[REGALLOC]** Add register coalescing to `LinearScanAllocator`.
- [ ] **[REGALLOC]** Add register pressure modeling to spill-cost heuristic.
- [ ] **[REGALLOC]** Re-enable `regalloc.rs:4066` `mod tests` (`#[cfg(any())] // Disabled: broken tests need fixing`).

---

# Wave 25 — Re-enable inliner

> `inline_small` is real but disabled at `opt.rs:1415`.

- [ ] **[OPT]** Fix the "caller never inlined" issue noted in the disabled comment. `src/codegen/src/opt.rs:1415`
- [ ] **[OPT-WIRE]** Re-enable `inline_small` in `run_optimizations_inner`. `src/codegen/src/opt.rs:1415`
- [ ] **[OPT]** Add an inline cost model (instruction count + call-arg count).
- [ ] **[OPT]** Add `inline_with_threshold` config knob in `CompileConfig`.
- [ ] **[TEST]** Add tests: inlining a small function reduces call count; recursive functions are not inlined infinitely.

---

# Wave 26 — Re-enable LICM

> `licm` is real but disabled because preheader blocks aren't emitted correctly.

- [ ] **[OPT]** Fix preheader block emission in codegen (the reason LICM was disabled). `src/codegen/src/opt.rs:1422`
- [ ] **[OPT-WIRE]** Re-enable `licm` in `run_optimizations_inner`. `src/codegen/src/opt.rs:1422`
- [ ] **[SCG-WIRE]** Add `LoopInvariantCodeMotion` to SCG `PassManager` (currently never added). `src/scg/src/transform.rs:1637`, `src/pipeline.rs:5862-5895`
- [ ] **[TEST]** Add tests: loop-invariant load is hoisted out of loop body.
- [ ] **[TEST]** Add tests: LICM doesn't hoist memory ops with possible aliasing.

---

# Wave 27 — Re-enable instruction scheduler

> `scheduler::schedule_function` is disabled at `opt.rs:1430` for pass-interaction miscompilation.

- [ ] **[OPT]** Stabilize the IR so CSE/LICM/inline produce scheduler-stable input (the root cause of the disable). `src/codegen/src/opt.rs:1430`
- [ ] **[OPT-WIRE]** Re-enable `scheduler::schedule_function` in `run_optimizations_inner`.
- [ ] **[SCHED]** Remove the memory-op bail-out at `scheduler.rs:122-132` and `:345-355` — model Load/Store dependencies properly.
- [ ] **[SCHED]** Add register-pressure modeling to list scheduling.
- [ ] **[SCHED]** Add per-backend `LatencyTable`.
- [ ] **[TEST]** Add tests: scheduled code produces same result as unscheduled; scheduling reduces critical-path length.

---

# Wave 28 — Re-enable cross-function constant prop & identical-function merge

- [ ] **[OPT]** Fix the constant-argument miscompilation in `cross_function_constant_prop`. `src/codegen/src/opt.rs:1443, 1562`
- [ ] **[OPT-WIRE]** Re-enable `cross_function_constant_prop` in `run_optimizations_inner`.
- [ ] **[OPT-WIRE]** Wire `identical_function_merge` (defined `opt.rs:1697`, never called).
- [ ] **[TEST]** Add tests: constants propagated into callees; identical functions merged.
- [ ] **[TEST]** Add regression tests for the original miscompilation.

---

# Wave 29 — Rewrite vectorizer

> `vectorize.rs` is a stub that miscompiles (blind 4× body duplication without
> IV adjustment). Delete and rewrite.

- [ ] **[OPT-DEL]** Delete `src/codegen/src/vectorize.rs` (the miscompiling stub).
- [ ] **[OPT]** Implement real SLP vectorization with a cost model.
- [ ] **[OPT]** Implement loop vectorization with IV-step adjustment (the exact thing the stub got wrong).
- [ ] **[BE-x86_64]** Emit SSE/AVX instructions from vectorized IR.
- [ ] **[BE-aarch64]** Emit NEON instructions from vectorized IR.
- [ ] **[TEST]** Add tests: `for i in 0..N { a[i] = b[i] + c[i]; }` lowers to a single vector loop.

---

# Wave 30 — Loop optimizer: multi-block unrolling & SCEV

> `loop_unroll.rs` bails on multi-block loops; hardcoded `UNROLL_FACTOR=2`; no
> trip-count analysis.

- [ ] **[OPT]** Implement multi-block loop unrolling with block-graph rewiring. `src/codegen/src/loop_unroll.rs:265-268`
- [ ] **[OPT]** Replace hardcoded `UNROLL_FACTOR=2` with trip-count-derived factor. `src/codegen/src/loop_unroll.rs:47`
- [ ] **[OPT]** Implement Scalar Evolution (SCEV) for trip-count analysis.
- [ ] **[OPT]** Implement unroll-and-jam (nested-loop optimization).
- [ ] **[OPT]** Add a code-size budget to the unroll heuristic.
- [ ] **[TEST]** Add tests: multi-block loops unroll correctly; trip-count-known loops fully unroll.

---

# Wave 31 — E-graph: rebuilding, extraction, and richer rules

> 16 identity-only rules; no rebuilding after merge; single-node extraction.

- [ ] **[EGRAPH]** Implement e-class rebuilding after merge (rehash parents). `src/codegen/src/egraph.rs`
- [ ] **[EGRAPH]** Replace single-node extraction with bottom-up DP extraction. `src/codegen/src/egraph.rs:222-235`
- [ ] **[EGRAPH]** Add commutativity rules (`+`, `*`, `&`, `|`, `^`).
- [ ] **[EGRAPH]** Add associativity rules.
- [ ] **[EGRAPH]** Add distributivity rules.
- [ ] **[EGRAPH]** Add constant-folding-across-ops rules (`(x + 0) + 0 → x`, etc.).
- [ ] **[TEST]** Add a rule-coverage test ensuring each new rule fires on a representative program.

---

# Wave 32 — Wire escape analysis & effects analysis

> Both real implementations, never called from the codegen pipeline.

- [ ] **[OPT-WIRE]** Wire `escape_analysis::analyze_escapes` into the pipeline. `src/codegen/src/escape_analysis.rs:31`
- [ ] **[OPT-WIRE]** Use escape analysis for scalar replacement of aggregates (SROA).
- [ ] **[OPT-WIRE]** Use escape analysis to elide `__vuma_alloc`/`__vuma_free` for non-escaping allocations.
- [ ] **[OPT-WIRE]** Wire `effects::analyze_program_effects`. `src/codegen/src/effects.rs:131`
- [ ] **[OPT]** Add interprocedural effect propagation (currently intra-function only). `src/codegen/src/effects.rs:131`
- [ ] **[TEST]** Add tests: non-escaping allocation is stack-promoted; pure functions are marked `Pure`.

---

# Wave 33 — Wire unused SCG-level passes

> `LoopInvariantCodeMotion`, `StrengthReduction`, `TailCallOptDetection`,
> `DeadRegionElimination` are defined and never added to `PassManager`.

- [ ] **[SCG-WIRE]** Add `LoopInvariantCodeMotion` to `PassManager` at O2+. `src/scg/src/transform.rs:1637`
- [ ] **[SCG-WIRE]** Add `StrengthReduction` to `PassManager` at O2+. `src/scg/src/transform.rs:1777`
- [ ] **[SCG-WIRE]** Add `TailCallOptDetection` to `PassManager` at O2+. `src/scg/src/transform.rs:1963`
- [ ] **[SCG-WIRE]** Add `DeadRegionElimination` to `PassManager` at O1+. `src/scg/src/transform.rs:2063`
- [ ] **[SCG]** Audit `scg/loop_detection.rs::LoopDetector` vs `regalloc::LoopDetector` — unify or document the split. `src/scg/src/loop_detection.rs:172`
- [ ] **[TEST]** Add tests: each pass fires on a representative SCG.

---

# Wave 34 — Wire lowering infrastructure: monomorphize, closures, switch/tail-call

> `monomorphize.rs`, `closures.rs`, `control_flow.rs::{SwitchLowerer,
> TailCallLowerer, LoopOptimizer}` — real, never called.

- [ ] **[LOWER-WIRE]** Wire `Monomorphizer` into the pipeline (currently only self-tested). `src/codegen/src/monomorphize.rs:33`
- [ ] **[LOWER-WIRE]** Wire `ClosureLowerer` into the pipeline. `src/codegen/src/closures.rs:56`
- [ ] **[LOWER-WIRE]** Wire `SwitchLowerer` into the pipeline. `src/codegen/src/control_flow.rs:74`
- [ ] **[LOWER-WIRE]** Wire `TailCallLowerer` into the pipeline. `src/codegen/src/control_flow.rs:833`
- [ ] **[LOWER-WIRE]** Wire `control_flow.rs::LoopOptimizer` (or document why production uses `loop_unroll` instead). `src/codegen/src/control_flow.rs:1735`
- [ ] **[TEST]** Add tests: a generic function is monomorphized; a closure is lowered to a function + environment struct.

---

# Wave 35 — Decide on exception & coroutine lowering

> `ExceptionLowerer` and `CoroutineLowerer` are real but the language may not
> have syntax for exceptions/coroutines. Decide: wire or delete.

- [ ] **[LOWER]** Audit whether `.vuma` has syntax for `try`/`catch`/`raise`. If not, decide on syntax.
- [ ] **[LOWER]** Audit whether `.vuma` has syntax for `async`/`await`/`yield`. If not, decide on syntax.
- [ ] **[LOWER-WIRE]** If keeping exceptions: wire `ExceptionLowerer`, add parser support, add tests. `src/codegen/src/control_flow.rs:597`
- [ ] **[LOWER-WIRE]** If keeping coroutines: wire `CoroutineLowerer`, add parser support, add tests. `src/codegen/src/control_flow.rs:1118`
- [ ] **[LOWER-DEL]** If deleting: remove `ExceptionLowerer` and `CoroutineLowerer`, remove dead tests.
- [ ] **[TEST]** Add end-to-end tests for whichever survives.

---

# Wave 36 — Wire proof log & `bv_verify` into the e-graph loop

> `ProofLog::record` is never called during `EGraph::saturate`;
> `check_proof_log` is never called outside its own tests.

- [ ] **[EGRAPH-WIRE]** Populate `ProofLog` during `EGraph::saturate` (record each rewrite application as a `ProofArtifact`). `src/codegen/src/proof_artifacts.rs:123`
- [ ] **[OPT-WIRE]** Wire `check_proof_log` as a compile-time check after e-graph saturation. `src/codegen/src/proof_artifacts.rs:127`
- [ ] **[OPT-WIRE]** Wire `bv_verify::verify_all_rules` as a gate before e-graph saturation (verify each rule is sound before applying). `src/codegen/src/bv_verify.rs:216`
- [ ] **[CI]** Add a CI step that runs `verify_all_rules` and fails the build on counterexample.
- [ ] **[TEST]** Add a test: an unsound rule is rejected by `bv_verify`.

---

# Wave 37 — CoR: make optimization passes real or delete

> All 4 CoR "optimization" passes are annotation-only (`is_inlined`,
> `unroll_factor`, etc.) and never transform node/edge structure.

- [ ] **[COR]** Implement real `HotPathInlining::apply` that copies callee body and redirects edges. `src/cor/src/optimization.rs:325-374`
- [ ] **[COR]** Implement real `ColdPathOutline::apply` that moves cold code to a new function. `src/cor/src/optimization.rs:412-504`
- [ ] **[COR]** Implement real `LoopOptimization::apply` that duplicates the body and adjusts IV. `src/cor/src/optimization.rs:598-706`
- [ ] **[COR]** Implement real `MemoryOptimization::apply` that emits prefetch and aligns data. `src/cor/src/optimization.rs:754-818`
- [ ] **[COR-DEL]** Alternatively, delete the 4 annotation-only passes and `OptimizationEngine` if CoR is to remain a profiling-only subsystem.
- [ ] **[TEST]** Add tests: each real pass transforms the SCG measurably.

---

# Wave 38 — CoR: wire `optimize()` into the pipeline

> `CORuntime::optimize()` is never called from the pipeline (only from tests).
> CoR is constructed at stage 11 *after* the binary is emitted.

- [ ] **[COR-WIRE]** Decide: (a) call `CORuntime::optimize()` from the pipeline and have CoR-compiled regions replace the user binary, or (b) document CoR as profiling-only and stop claiming it optimizes user code.
- [ ] **[COR-WIRE]** If (a): move CoR construction before binary emission; have `emit_binary` consume CoR-compiled regions.
- [ ] **[COR-WIRE]** Wire `SpeculativeOptimizer::validate_all` into the pipeline. `src/cor/src/speculative.rs:219`
- [ ] **[COR-WIRE]** Make `apply_speculation` produce real speculative code (currently caller-provided only). `src/cor/src/speculative.rs:891`
- [ ] **[COR]** Stop compiling synthetic stubs from SCG metadata in `runtime.rs:580-660` (they don't represent user code).
- [ ] **[TEST]** Add end-to-end test: CoR optimization measurably changes the emitted binary.

---

# Wave 39 — Self-hosting: hand-written `DiGraph` in `scg/src/graph.rs` (1/2)

> Petgraph is the actual backing store of the SCG. Replace it.

- [ ] **[SCG]** Implement hand-written `DiGraph` (linked-list adjacency, matching `womb/graph/digraph.vuma` design) in a new `src/scg/src/digraph.rs`.
- [ ] **[SCG]** Implement the 17 storage methods currently delegated to petgraph (`add_node`, `add_edge`, `remove_node`, …). `src/scg/src/graph.rs`
- [ ] **[SCG]** Implement hand-written `toposort` (Kahn's algorithm).
- [ ] **[SCG]** Implement hand-written `tarjan_scc` (copy pattern from `src/ive/src/liveness.rs:723-749`).
- [ ] **[SCG]** Implement hand-written `has_path_connecting` (BFS).
- [ ] **[SCG]** Replace petgraph usage in `src/scg/src/graph.rs:9-12` with the new `DiGraph`.

---

# Wave 40 — Self-hosting: hand-written `DiGraph` (2/2) — remove petgraph dep

- [ ] **[SCG]** Remove `petgraph` from `src/scg/Cargo.toml:15`.
- [ ] **[SCG]** Remove `petgraph` from workspace `Cargo.toml:53,93`.
- [ ] **[SCG]** Audit `src/codegen/src/scg_to_ir.rs` (declares petgraph dep but defines its own stub `Scg`) — remove the spurious dep.
- [ ] **[SCG]** Audit `src/scg/src/serialize.rs` for petgraph references — remove.
- [ ] **[SCG]** Verify `womb/graph/digraph.vuma` matches the new Rust `DiGraph` API (so the VUMA-native version can later replace the Rust one).
- [ ] **[TEST]** Add SCG conformance tests: every algorithm produces identical results to the old petgraph-backed version.

---

# Wave 41 — Self-hosting: replace `indexmap`, `smallvec`, `thiserror`

- [ ] **[SCG]** Replace `indexmap::IndexSet<NodeId>` (2 sites) with `HashSet<NodeId>` + `Vec<NodeId>` for order. `src/scg/src/graph.rs:684,690`
- [ ] **[SCG]** Remove `indexmap` from `src/scg/Cargo.toml`.
- [ ] **[SCG]** Replace `smallvec::SmallVec<[NodeId; 8]>` (6 sites) with `Vec<NodeId>`. `src/scg/src/query.rs:18`
- [ ] **[SCG]** Remove `smallvec` from `src/scg/Cargo.toml`.
- [ ] **[CORE]** Replace `#[derive(thiserror::Error)]` (~15 sites) with hand-written `Display`/`Error` impls (pattern at `src/scg/src/graph.rs:44-64`).
- [ ] **[CORE]** Remove `thiserror` from workspace `Cargo.toml:54`.

---

# Wave 42 — Self-hosting: unify `hashbrown` vs `std::collections::HashMap`

> IVE and vuma-core mix `hashbrown::HashMap` and `std::collections::HashMap`/
> `BTreeMap`/`BTreeSet`/`VecDeque` in the same crate.

- [ ] **[IVE]** Audit `hashbrown::HashMap` vs `std::collections::HashMap` usage in `src/ive/src/`.
- [ ] **[CORE]** Audit same in `src/vuma/src/`.
- [ ] **[CORE]** Unify to a single `HashMap` type across all core crates (recommend `std::collections::HashMap` as the bootstrap-time substrate).
- [ ] **[CORE]** Implement a VUMA-native `HashMap` in `womb/collections/hashmap.vuma` matching the unified API.
- [ ] **[CORE]** Remove `hashbrown` from core crate `Cargo.toml`s (keep only where genuinely needed for perf, with a TODO).
- [ ] **[TEST]** Add a conformance test: unified `HashMap` produces identical results to `hashbrown::HashMap`.

---

# Wave 43 — Self-hosting: strip `serde` derives from core compiler types

> ~494 `#[derive(Serialize, Deserialize)]` sites in core crates. Keep serde
> only for peripheral JSON (LLM API, telemetry, LSP).

- [ ] **[CORE]** Audit all `#[derive(Serialize, Deserialize)]` in `src/scg/`, `src/ive/`, `src/bd/`, `src/vuma/`, `src/proof/`, `src/codegen/`.
- [ ] **[SCG]** Replace serde derives on `NodeData`/`EdgeData`/`SCGRegion` with hand-written binary (de)serialization via `src/scg/src/serialize.rs` `BinaryReader`/`BinaryWriter`.
- [ ] **[BD]** Replace serde derives on `RepD`/`CapD`/`RelD`/`BD` with hand-written binary (de)serialization.
- [ ] **[PROOF]** Replace serde derives on proof artifacts with hand-written binary (de)serialization.
- [ ] **[CORE]** Remove `serde`/`serde_json` from core crate `Cargo.toml`s. Keep in `src/llm_api.rs`, `src/telemetry.rs`, `src/lsp/` only.
- [ ] **[TEST]** Add round-trip tests for each hand-written serializer.

---

# Wave 44 — Self-hosting: replace `log` crate with VUMA-native macro

> 129 `log::debug!`/`info!`/`warn!`/`trace!`/`error!` call sites in core.

- [ ] **[CORE]** Define a `vuma_log!` macro in `src/lib.rs` (or a new `src/log.rs`).
- [ ] **[CORE]** Mechanically replace `log::debug!` → `vuma_log!(debug, …)` across all core crates.
- [ ] **[CORE]** Same for `info!`, `warn!`, `trace!`, `error!`.
- [ ] **[CORE]** Remove `log` from core crate `Cargo.toml`s.
- [ ] **[CORE]** Implement `vuma_log!` as a no-op when `--release` is set, real logging otherwise.
- [ ] **[TEST]** Verify log output is unchanged.

---

# Wave 45 — Self-hosting: remove `libc` from COR runtime & vuma-std

- [ ] **[COR]** Replace `libc::mmap`/`mprotect`/`munmap` in `src/cor/src/runtime.rs:1022-1141` with direct syscalls via `extern "C" { fn mmap(...); }` (which VUMA already supports).
- [ ] **[COR]** Add a non-Unix fallback that returns a clear error (not silent `Ok(0)`). `src/cor/src/runtime.rs:1003-1007`
- [ ] **[STD]** Replace `libc::malloc`/`free`/`realloc` in `src/std/src/alloc.rs:2010,2067,2119` with `__vuma_alloc`/`__vuma_free` (already used by `womb/graph/digraph.vuma`).
- [ ] **[STD]** Replace `libc::read`/`write` (8 sites) in `src/std/src/io.rs` with direct syscalls.
- [ ] **[STD]** Remove `libc` from `src/std/Cargo.toml`. Remove the `os-linux` feature gate.
- [ ] **[TEST]** Add tests: COR JIT executes a compiled region without libc; vuma-std I/O works without libc.

---

# Wave 46 — Self-hosting: wire `vuma-std` as compiler substrate (or mark runtime-only)

> `vuma-std` (24K LOC) is depended on by zero other crates. Decide its role.

- [ ] **[STD-DECIDE]** Decide: (a) wire `vuma-std` as a dependency of `vuma-core` and migrate to `VumaVec`/`VumaHashMap`/`VumaString`, or (b) mark as runtime-only.
- [ ] **[STD-WIRE]** If (a): add `vuma-std` to `vuma-core`'s deps; migrate `Vec<T>` → `VumaVec<T>` in core types.
- [ ] **[STD-WIRE]** If (a): migrate `HashMap` → `VumaHashMap`; migrate `String` → `VumaString`.
- [ ] **[STD-ABI]** Define a shared syscall ABI between `vuma-std` (which currently calls libc) and the codegen backends (which emit raw syscall stubs). Currently they're decoupled.
- [ ] **[STD-DOC]** If (b): update `src/std/src/lib.rs` doc-comment to say "runtime library for VUMA programs, not the compiler's substrate".
- [ ] **[TEST]** Add tests: a `.vuma` program can call `vuma-std` functions and get verified behavior.

---

# Wave 47 — Bootstrap: consolidate `womb/lang/` drafts

> 5+ parallel incomplete compiler drafts (4,283 LOC), none wired in.

- [ ] **[BOOT]** Audit each `womb/lang/*.vuma` file: `vuma_compiler.vuma` (505), `mini_compiler.vuma` (206), `minicompiler.vuma` (103), `full_lexer.vuma`+`full_parser.vuma`+`ir_builder.vuma`+`codegen.vuma`+`elf.vuma`, `self_host_test.vuma` (206).
- [ ] **[BOOT]** Pick ONE as canonical (recommend `full_lexer`+`full_parser`+`ir_builder`+`codegen`+`elf` as the pipeline).
- [ ] **[BOOT-DEL]** Delete the other drafts (`mini_compiler.vuma`, `minicompiler.vuma`, `vuma_compiler.vuma`).
- [ ] **[BOOT]** Add file I/O via `extern "C" { fn read(...); }` to the bootstrap compiler (currently source is hardcoded).
- [ ] **[BOOT]** Add argv parsing so the bootstrap compiler reads a `.vuma` file from disk.
- [ ] **[TEST]** Add a test: the bootstrap compiler reads `womb/lang/hello.vuma` and produces exit code 0.

---

# Wave 48 — Bootstrap: full pipeline (lexer → parser → AST → IR → codegen → ELF)

> Currently `src/bootstrap/vuma_compiler.vuma` (730 LOC) is a lexer POC that
> hardcodes a 47-byte input.

- [ ] **[BOOT]** Implement a real parser (not just `lex_next_token`) in the bootstrap compiler.
- [ ] **[BOOT]** Implement AST construction.
- [ ] **[BOOT]** Implement SCG construction from AST.
- [ ] **[BOOT]** Implement BD inference (at least Phase 1 propagation).
- [ ] **[BOOT]** Implement IVE verification (at least liveness + cleanup).
- [ ] **[BOOT]** Implement IR construction from SCG.
- [ ] **[BOOT]** Implement x86_64 codegen (reuse encoders from `womb/lang/codegen.vuma`).
- [ ] **[BOOT]** Implement ELF64 emission (reuse `womb/lang/elf.vuma`).
- [ ] **[BOOT-SELF]** Self-host: bootstrap compiler compiles `womb/lang/hello.vuma` and the resulting binary runs correctly.

---

# Wave 49 — Wrapper-backend documentation & cross-backend conformance

- [ ] **[BE-aarch64_be]** Document the byte-swap wrapper pattern in `src/codegen/src/aarch64_be.rs:1-20`.
- [ ] **[BE-armeb]** Document BE32 mode word-swap in `src/codegen/src/armeb.rs:1-20`.
- [ ] **[BE-mips64be]** Document byte-swap in `src/codegen/src/mips64be.rs:1-20`.
- [ ] **[BE-ppc64le]** Document ELFv2 vs ELFv1 ABI flag in `src/codegen/src/ppc64le.rs:1-20`.
- [ ] **[TEST]** Add a cross-backend syscall conformance test: every backend emits the same set of named syscalls.
- [ ] **[TEST]** Add a `print_int`/`print_hex`/`print_newline` regression test for all 19 backends.

---

# Wave 50 — Final hardening: real-regalloc correctness, IVE-proof end-to-end, memory-safety blocking

- [ ] **[TEST]** Add real-regalloc correctness test per backend (SHA256d, mmap_sha256d).
- [ ] **[TEST]** Add IVE-proof-system end-to-end test: a verified program produces a non-empty `ProofBundle` with `ProofChecker::check == Valid`.
- [ ] **[TEST]** Add memory-safety-blocking regression test: a UAF program is rejected at compile time.
- [ ] **[TEST]** Add cross-backend optimization regression: same program produces same observable behavior on every backend.
- [ ] **[TEST]** Add self-hosting milestone test: bootstrap compiler compiles a non-trivial `.vuma` file.
- [ ] **[CI]** Add a CI step that runs `bun run lint` (or `cargo clippy`) and fails on warnings.
- [ ] **[CI]** Add a CI step that runs the full test suite on every push.

---

## Cross-cutting conventions

- Every code-changing task MUST be accompanied by at least one test in `src/tests/`.
- Every backend change MUST be verified by `cargo test --workspace` before merge.
- Every deletion (`[*-DEL]`) MUST be preceded by a grep proving zero production callers.
- Every wiring (`[*-WIRE]`) MUST be accompanied by an end-to-end test exercising the newly-wired path.
- Every Wave MUST be committed as a single squashed commit (or a small stack) with message `wave(N): <summary>`.

## Wave dependency graph (simplified)

```
1 ──► 2 ──► 3 ──► 49 (print consistency)
4 ──► 5 (wasm32 WASI)
6 (mmap ABI)
7 ──► 8 ──► 9 (POSIX syscalls)
10 ──► 11 ──► 12 ──► 13 (IR Syscall)
14 ──► 15 (dead IVE cleanup) ──► 16 (wire IVE hardened)
17 ──► 18 (proof system) ──► 19 (escape hatches)
20 (memory safety blocking)
21 ──► 22 ──► 23 ──► 24 (real regalloc)
25, 26, 27, 28 (re-enable opts, parallel after 21)
29, 30 (vectorize + loop, parallel after 25)
31 (e-graph, parallel)
32, 33 (wire analyses, parallel after 21)
34, 35 (lowering, parallel after 21)
36 (proof log, after 31)
37 ──► 38 (CoR, parallel after 21)
39 ──► 40 (petgraph removal)
41, 42, 43, 44 (self-hosting deps, parallel after 40)
45 (libc removal, after 39)
46 (vuma-std, after 45)
47 ──► 48 (bootstrap, after 39-46)
50 (final hardening, after all)
```
