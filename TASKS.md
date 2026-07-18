# VWK Refinement Spec — Universal 19-Arch Abstractions

> **Goal:** Refine the VWK kernel's abstractions so that the core logic
> (scheduler, PMM, VFS, syscall dispatch, trap handler) is **100%
> architecture-independent PMT** — zero `#ifdef`, zero arch-conditional
> branches. Architecture differences are pushed to the absolute edge
> (HAL transforms + minimal assembly trampolines) and the core compiles
> identically for all 19 backends.

## Golden Rule

**Do not write arch-conditional logic inside kernel transforms.**
If you find yourself writing `if arch == AArch64` inside a scheduler,
PMM, or VFS transform, you have failed the PMT model.

Instead:
1. Define **generic superset PMT layouts** (TrapFrame, BootInfo, etc.)
2. Write **arch-independent core** (manipulates offsets in the layouts)
3. Write **arch-specific HAL transforms** (one file per arch, same interface)
4. Write **minimal assembly trampolines** (boot entry, IRQ entry, context switch)

---

## Current State (K0–K12 Complete)

The first 13 waves (K0–K12) built the full kernel: 75 PMT-pure `.vuma`
files, 19-backend parity verified (190/190 gold tests + 76/76 module
compiles pass). The architecture follows the 4-layer cake:
- L1: boot.S (stubbed in hosted mode)
- L2: FFI trampolines (extern "C" stubs)
- L3: Arena runtime (mmap/mremap/munmap)
- L4: PMT kernel logic (arch-independent)

**What needs refinement:** 3 abstractions leak arch-coupling into the
core:
1. TrapFrame is per-arch (3 different layouts with different fields)
2. BootInfo is per-arch (3 different layouts)
3. EarlyConsole has per-arch functions instead of a unified interface

Plus 2 new subsystems needed:
4. IRQ Ring Buffer (decouples asm IRQ entry from PMT dispatch)
5. Real VMM (replace extern stubs with real PMT page-table walkers)

---

## Wave R1 — Universal Boot & Early Console

**Goal:** Define a single superset `BootInfo` layout and a single
`EarlyConsole` interface that all archs populate. Update `kmain` to
read from the superset, not arch-specific fields.

**Max parallel:** 4 (R1a ∥ R1b ∥ R1c ∥ R1d), then R1e after all).

### R1a — Superset BootInfo + x86_64 bootinfo_init

**Files:** `womb/kernel/bootinfo.vuma` (new, canonical), `womb/kernel/arch/x86_64/bootinfo.vuma` (rewrite)

**Contract:**
- Define the canonical superset `layout BootInfo` in `womb/kernel/bootinfo.vuma`:
  ```
  layout BootInfo = {
      mem_start: u64,        // start of usable RAM
      mem_size: u64,         // size of usable RAM in bytes
      cmdline: u64,          // address of command-line string (or 0)
      dtb_addr: u64,         // device tree blob address (or 0)
      multiboot_magic: u64,  // multiboot2 magic (x86_64 only, 0 otherwise)
      framebuffer: u64,      // framebuffer MMIO base (or 0)
      fb_width: u32,         // framebuffer width in pixels
      fb_height: u32,        // framebuffer height in pixels
      boot_type: u8,         // 0=x86_multiboot, 1=dtb, 2=sbi, 3=wasm, 4=ipl
      _pad: [u8; 7],         // alignment padding to 8 bytes
  }
  ```
- Write `fn bootinfo_init(bi: State<BootInfo>)` that zeros all fields.
- Rewrite `womb/kernel/arch/x86_64/bootinfo.vuma` to re-declare the
  superset BootInfo (byte-identical) and provide
  `fn x86_bootinfo_init(bi: State<BootInfo>, magic: u64, mbi_addr: u64)`
  that sets `boot_type=0`, `multiboot_magic=magic`, and parses the
  multiboot2 memory map into `mem_start`/`mem_size` (stub: set defaults).
- Each file must have `fn main() -> i32 { return 0; }`.

**DoD:**
- [ ] `womb/kernel/bootinfo.vuma` exists with the superset BootInfo layout
- [ ] `bootinfo_init` zeros all fields
- [ ] `womb/kernel/arch/x86_64/bootinfo.vuma` re-declares superset + `x86_bootinfo_init`
- [ ] Both files compile + verify on x86_64
- [ ] No pointer syntax

### R1b — aarch64 bootinfo_init

**Files:** `womb/kernel/arch/aarch64/bootinfo.vuma` (rewrite)

**Contract:**
- Re-declare the superset BootInfo layout (byte-identical to R1a).
- Write `fn arm64_bootinfo_init(bi: State<BootInfo>, dtb_addr: u64, mem_start: u64, mem_size: u64)`:
  sets `boot_type=1`, `dtb_addr=dtb_addr`, `mem_start=mem_start`, `mem_size=mem_size`.
- `fn main() -> i32 { return 0; }`

**DoD:**
- [ ] File re-declares superset BootInfo + `arm64_bootinfo_init`
- [ ] Compiles + verifies on x86_64 (cross-compile test)
- [ ] No pointer syntax

### R1c — riscv64 bootinfo_init

**Files:** `womb/kernel/arch/riscv64/bootinfo.vuma` (rewrite)

**Contract:**
- Re-declare the superset BootInfo layout (byte-identical to R1a).
- Write `fn riscv_bootinfo_init(bi: State<BootInfo>, hartid: u64, dtb_addr: u64, mem_start: u64, mem_size: u64)`:
  sets `boot_type=2`, `dtb_addr=dtb_addr`, `mem_start=mem_start`, `mem_size=mem_size`.
- `fn main() -> i32 { return 0; }`

**DoD:**
- [ ] File re-declares superset BootInfo + `riscv_bootinfo_init`
- [ ] Compiles + verifies on x86_64
- [ ] No pointer syntax

### R1d — Universal EarlyConsole

**Files:** `womb/kernel/drivers/early_console.vuma` (new)

**Contract:**
- Define `layout EarlyConsole = { base: u64, con_type: u8, _pad: [u8; 7] }`
  where `con_type`: 0=16550 UART, 1=PL011 UART, 2=semihosting, 3=diag (s390x), 4=wasm host callback.
- Declare externs: `fn mmio_write8(addr: u64, val: u8)`, `fn mmio_write32(addr: u64, val: u32)`, `fn write(fd: i64, buf: Address, count: i64) -> i64`.
- Write `fn early_console_init(ec: State<EarlyConsole>, base: u64, con_type: u8)`.
- Write `fn early_console_write_byte(ec: State<EarlyConsole>, c: u8)`:
  - type 0 (16550): `mmio_write8(ec.base, c)` (also poll LSR for TX empty — stub in hosted)
  - type 1 (PL011): `mmio_write32(ec.base, c as u32)` (also poll FR for TX FIFO — stub)
  - type 4 (wasm): `write(1, buf, 1)` using a temp buffer
  - other types: no-op (stub)
- Write `fn early_console_write_str(ec: State<EarlyConsole>, s: State<ConsoleStr>)`:
  iterate over `s.data[0..N]` until null, call `write_byte` for each.
- `layout ConsoleStr = { data: [u8; 256] }`
- `fn main() -> i32 { return 0; }`

**DoD:**
- [ ] EarlyConsole layout + ConsoleStr layout verify
- [ ] `early_console_init`, `early_console_write_byte`, `early_console_write_str` verify
- [ ] Self-test: init type 4 (wasm), write "Hi\n", verify no crash, exit 0
- [ ] No pointer syntax

### R1e — Update kmain + console for superset BootInfo + EarlyConsole

**Files:** `womb/kernel/kernel.vuma` (rewrite), `womb/kernel/console.vuma` (update)

**Depends on:** R1a, R1b, R1c, R1d complete.

**Contract:**
- Re-declare superset BootInfo + EarlyConsole + ConsoleStr (byte-identical).
- `kernel.vuma`: `fn main()` calls `kmain()`. `kmain` takes no args (BootInfo
  is populated by the boot stub before main). kmain calls:
  1. `early_console_init(ec, 1016, 0)` — x86_64 16550 at COM1 (hosted: type 4)
  2. Print "vuma kernel: hello\n" via `early_console_write_byte`
  3. Halt
- `console.vuma`: update to use `EarlyConsole` instead of the old
  `Console` layout. Keep `console_putc`/`console_flush` but route through
  `early_console_write_byte`.
- `fn main() -> i32 { return 0; }` in each file.

**DoD:**
- [ ] `kernel.vuma` compiles + verifies + runs on x86_64, prints banner, exits 0
- [ ] `console.vuma` compiles + verifies on x86_64
- [ ] `scripts/kernel_smoke.sh` still passes
- [ ] No pointer syntax

---

## Wave R2 — Universal Trap & IRQ

**Goal:** Define a single superset `TrapFrame` layout that all archs
populate. Add a lock-free IRQ ring buffer that decouples the asm IRQ
entry from the PMT dispatcher. Update the trap handler to be 100%
arch-independent.

**Max parallel:** 5 (R2a ∥ R2b ∥ R2c ∥ R2d ∥ R2e), then R2f after all).

### R2a — Superset TrapFrame + canonical register mapping

**Files:** `womb/kernel/trap/trap_frame.vuma` (new, canonical)

**Contract:**
- Define the canonical superset layout:
  ```
  layout TrapFrame = {
      gpr: [u8; 256],       // 32 × 8 bytes — general-purpose registers
      pc: u64,               // program counter (rip / elr_el1 / mepc)
      status: u64,           // status register (rflags / spsr_el1 / mstatus)
      vector: u64,           // interrupt vector / exception cause
      error_code: u64,       // CPU error code (x86) / esr_el1 (ARM) / stval (RISC-V)
      sp: u64,               // stack pointer at trap entry
      fpu: [u8; 256],        // 32 × 8 bytes — FPU/SIMD registers (optional)
      _pad: [u8; 24],        // align to 8 bytes, total = 552 bytes
  }
  ```
  The `gpr` array uses flat `[u8; 256]` (VUMA array index is byte-granular).
  Each arch maps its registers to canonical slots:
  - Slot 0-15: x86_64 rax, rbx, rcx, rdx, rsi, rdi, rbp, r8-r15
  - Slot 0-30: aarch64 x0-x30
  - Slot 0-30: riscv64 ra, sp, gp, tp, t0-t6, s0-s11, a0-a7
  Unused slots are zero.
- Write pack/unpack helpers:
  `fn tf_get_gpr(tf: State<TrapFrame>, idx: u32) -> u64`
  `fn tf_set_gpr(tf: State<TrapFrame>, idx: u32, val: u64)`
  (8-byte LE pack/unpack at `idx * 8` in the `gpr` array.)
- Write `fn tf_init(tf: State<TrapFrame>)` that zeros all fields.
- `fn main() -> i32` self-test: init, set gpr[0]=42, verify gpr[0]==42, exit 0.

**DoD:**
- [ ] `trap_frame.vuma` exists with superset TrapFrame
- [ ] `tf_get_gpr`, `tf_set_gpr`, `tf_init` verify
- [ ] Self-test exits 0
- [ ] No pointer syntax

### R2b — x86_64 trap_trampoline for superset TrapFrame

**Files:** `womb/kernel/arch/x86_64/trap_trampoline.vuma` (rewrite)

**Contract:**
- Re-declare superset TrapFrame + helpers (byte-identical to R2a).
- Write `fn x86_trap_to_superset(old: State<X86TrapFrame>, sup: State<TrapFrame>)`:
  copies the 15 x86_64 GPRs into superset gpr[0..14], sets pc=old.rip,
  status=old.rflags, vector=old.vector, error_code=old.error_code, sp=old.rsp.
- `layout X86TrapFrame` = the old x86_64 TrapFrame (keep for compatibility).
- Keep externs: `idt_load`, `irq_mask`, `irq_unmask`, `pic_eoi`, `cr2_read`.
- `fn main() -> i32` self-test: create old + sup, copy, verify sup.gpr[0]==old.rax.

**DoD:**
- [ ] File compiles + verifies on x86_64, self-test exits 0
- [ ] `x86_trap_to_superset` correctly maps all fields
- [ ] No pointer syntax

### R2c — aarch64 trap_trampoline for superset TrapFrame

**Files:** `womb/kernel/arch/aarch64/trap_trampoline.vuma` (rewrite)

**Contract:**
- Re-declare superset TrapFrame + helpers (byte-identical to R2a).
- Write `fn arm64_trap_to_superset(old: State<Arm64TrapFrame>, sup: State<TrapFrame>)`:
  copies x0-x30 into gpr[0..30], sets pc=old.elr_el1, status=old.spsr_el1,
  vector=trap_exception_class(old), error_code=old.esr_el1, sp=old.sp_el0.
- `layout Arm64TrapFrame` = the old aarch64 TrapFrame (keep for compat).
- Keep externs: `vbar_load`, `gic_eoi`, `gic_mask`, `gic_unmask`, `far_el1_read`.
- `fn main() -> i32` self-test: create old + sup, copy, verify sup.gpr[0]==old.x0.

**DoD:**
- [ ] File compiles + verifies on x86_64, self-test exits 0
- [ ] `arm64_trap_to_superset` correctly maps all fields
- [ ] No pointer syntax

### R2d — riscv64 trap_trampoline for superset TrapFrame

**Files:** `womb/kernel/arch/riscv64/trap_trampoline.vuma` (rewrite)

**Contract:**
- Re-declare superset TrapFrame + helpers (byte-identical to R2a).
- Write `fn riscv_trap_to_superset(old: State<RiscVTrapFrame>, sup: State<TrapFrame>)`:
  copies ra,t0-t6,s0-s11,a0-a7 into gpr[0..30] (canonical order), sets
  pc=old.mepc, status=old.mstatus, vector=old.scause, error_code=old.stval, sp=old.sp.
- `layout RiscVTrapFrame` = the old riscv64 TrapFrame (keep for compat).
- Keep externs: `stvec_load`, `plic_eoi`, `plic_mask`, `plic_unmask`, `stval_read`.
- `fn main() -> i32` self-test: create old + sup, copy, verify.

**DoD:**
- [ ] File compiles + verifies on x86_64, self-test exits 0
- [ ] `riscv_trap_to_superset` correctly maps all fields
- [ ] No pointer syntax

### R2e — IRQ Ring Buffer

**Files:** `womb/kernel/trap/irq_ring.vuma` (new)

**Contract:**
- Define `layout IrqRing = { buf: [u8; 256], head: u32, tail: u32, count: u32 }`
  where `buf` is a flat array of 32 × 8-byte u64 IRQ vectors.
- Write `fn irq_ring_init(ring: State<IrqRing>)` — head=tail=count=0.
- Write `fn irq_ring_push(ring: State<IrqRing>, vector: u64) -> i64`:
  if count < 32, write vector at tail, advance tail (& 31), count++.
  Returns 0 on success, -1 if full.
- Write `fn irq_ring_pop(ring: State<IrqRing>) -> u64`:
  if count > 0, read vector at head, advance head (& 31), count--.
  Returns vector, or 9999 if empty.
- Pack/unpack helpers for the 8-byte u64 vectors in the flat buf array.
- `fn main() -> i32` self-test: push 3 vectors, pop 3, verify FIFO order.

**DoD:**
- [ ] `irq_ring.vuma` exists
- [ ] IrqRing layout + push/pop/init verify
- [ ] Self-test exits 0 (push 3, pop 3, verify order)
- [ ] No pointer syntax

### R2f — Update trap dispatcher + irq dispatcher for superset

**Files:** `womb/kernel/trap/trap.vuma` (rewrite), `womb/kernel/trap/irq.vuma` (update)

**Depends on:** R2a, R2b, R2c, R2d, R2e complete.

**Contract:**
- `trap.vuma`: re-declare superset TrapFrame + helpers. Rewrite
  `fn trap_handler(tf: State<TrapFrame>)` to use the superset:
  - `tf.vector < 32` → CPU exception → `trap_panic(tf)`
  - `tf.vector == 128` → syscall → `trap_syscall(tf)` (stub)
  - else → IRQ → `trap_irq(tf)`
  All field access via `tf_get_gpr(tf, N)` / `tf.vector` / `tf.pc` etc.
- `irq.vuma`: re-declare IrqRing + push/pop. Update `fn irq_dispatch_loop(ring: State<IrqRing>, tbl: State<IrqTable>)`:
  loop: `let v = irq_ring_pop(ring); if v == 9999 { return; } ... dispatch v ... goto loop`.
- Both files: `fn main() -> i32` self-test.

**DoD:**
- [ ] `trap.vuma` compiles + verifies, self-test exits 0
- [ ] `irq.vuma` compiles + verifies, self-test exits 0
- [ ] Trap handler uses ONLY superset TrapFrame fields (no arch-specific fields)
- [ ] IRQ dispatcher uses ring buffer (no direct function-pointer dispatch)
- [ ] No pointer syntax

---

## Wave R3 — Universal Memory Management

**Goal:** Simplify the PMM (keep buddy but audit for arch-coupling).
Define a generic `VmmNode` layout and arch-specific page-table walkers
that replace the extern "C" pte_read/pte_write stubs.

**Max parallel:** 3 (R3a ∥ R3b ∥ R3c), then R3d ∥ R3e ∥ R3f after R3b).

### R3a — PMM audit + arch-coupling removal

**Files:** `womb/kernel/mm/pmm.vuma` (audit + minor fixes)

**Contract:**
- Read the current `pmm.vuma`. Verify it contains ZERO arch-specific
  code (no `if arch == ...`, no externs, no arch-specific constants).
- If any arch-coupling is found, remove it.
- Add a header comment documenting that the PMM is 100% arch-independent.
- Verify the self-test still passes.

**DoD:**
- [ ] `pmm.vuma` contains zero arch-specific code
- [ ] Self-test still exits 0
- [ ] Header comment documents arch-independence

### R3b — Generic VmmNode + map_page dispatcher

**Files:** `womb/kernel/mm/vmm.vuma` (rewrite)

**Contract:**
- Define `layout VmmNode = { entries: [u8; 4096], level: u8, is_leaf: u8 }`
  — a generic page-table node (4096 bytes = one page).
- Define `layout VmmSpace = { root: u64, levels: u8, arch: u8 }`
  — root physical address, number of levels, arch ID (0=x86_64, 1=aarch64, 2=riscv64).
- Write `fn vmm_map_page(space: State<VmmSpace>, vaddr: u64, paddr: u64, flags: u64)`:
  dispatches to arch-specific walker based on `space.arch`:
  - arch 0 → call `x86_map_page` (extern stub for now)
  - arch 1 → call `arm_map_page` (extern stub)
  - arch 2 → call `riscv_map_page` (extern stub)
  This is the ONE place arch-dispatch is allowed (the HAL boundary).
- Write `fn vmm_unmap_page(space: State<VmmSpace>, vaddr: u64)`.
- Write `fn vmm_translate(space: State<VmmSpace>, vaddr: u64) -> u64`.
- Keep the old `vmm_walk_idx` helper for self-test.
- `fn main() -> i32` self-test.

**DoD:**
- [ ] VmmNode + VmmSpace layouts verify
- [ ] `vmm_map_page`, `vmm_unmap_page`, `vmm_translate` verify
- [ ] Arch-dispatch is in `vmm_map_page` only (documented HAL boundary)
- [ ] Self-test exits 0
- [ ] No pointer syntax

### R3c — kmalloc/mmap audit

**Files:** `womb/kernel/mm/kmalloc.vuma` (audit), `womb/kernel/mm/mmap.vuma` (audit)

**Contract:**
- Verify both files contain zero arch-specific code.
- Add header comments documenting arch-independence.
- Verify self-tests still pass.

**DoD:**
- [ ] Both files contain zero arch-specific code
- [ ] Self-tests still exit 0

### R3d — x86_64 page-table walker (PMT transforms)

**Depends on:** R3b complete.

**Files:** `womb/kernel/arch/x86_64/vmm_hal.vuma` (new)

**Contract:**
- Re-declare VmmNode + VmmSpace (byte-identical to R3b).
- Re-declare the x86_64 PageTableEntry layout from K2c.
- Write `fn x86_map_page(space: State<VmmSpace>, vaddr: u64, paddr: u64, flags: u64)`:
  4-level walk (PML4→PDPT→PD→PT). At each level, compute the 9-bit index
  from vaddr, check if the entry is present, allocate a new node if not
  (stub: return 0 on missing), and descend. At the leaf, write the PTE.
- Write `fn x86_translate(space: State<VmmSpace>, vaddr: u64) -> u64`:
  walk to leaf, return paddr.
- Use `extern "C"` for `pte_read`/`pte_write` (real MMIO in bare-metal).
- `fn main() -> i32` self-test.

**DoD:**
- [ ] File compiles + verifies on x86_64
- [ ] `x86_map_page` + `x86_translate` verify
- [ ] 4-level walk logic is correct (9-bit index extraction per level)
- [ ] No pointer syntax

### R3e — aarch64 page-table walker

**Depends on:** R3b complete.

**Files:** `womb/kernel/arch/aarch64/vmm_hal.vuma` (new)

**Contract:**
- Same as R3d but for ARMv8 4-level paging (4KB granule).
- `fn arm_map_page(space: State<VmmSpace>, vaddr: u64, paddr: u64, flags: u64)`.
- Use the aarch64 PTE layout from K2d.

**DoD:**
- [ ] File compiles + verifies on x86_64
- [ ] `arm_map_page` + `arm_translate` verify
- [ ] No pointer syntax

### R3f — riscv64 page-table walker

**Depends on:** R3b complete.

**Files:** `womb/kernel/arch/riscv64/vmm_hal.vuma` (new)

**Contract:**
- Same as R3d but for RISC-V Sv39 3-level paging.
- `fn riscv_map_page(space: State<VmmSpace>, vaddr: u64, paddr: u64, flags: u64)`.
- Use the riscv64 PTE layout from K2e.

**DoD:**
- [ ] File compiles + verifies on x86_64
- [ ] `riscv_map_page` + `riscv_translate` verify
- [ ] No pointer syntax

---

## Wave R4 — Universal Scheduling & Syscall

**Goal:** Make the scheduler 100% arch-independent using the superset
TrapFrame. Unify syscall dispatch. Handle wasm32's cooperative scheduling.

**Max parallel:** 4 (all parallel).

### R4a — Arch-independent scheduler using superset TrapFrame

**Files:** `womb/kernel/proc/scheduler.vuma` (rewrite)

**Contract:**
- Re-declare superset TrapFrame + helpers (byte-identical to R2a).
- Re-declare Runqueue + ProcessTable + helpers (byte-identical to K4a/b).
- Write `fn schedule(rq: State<Runqueue>, tbl: State<ProcessTable>, current_tf: State<TrapFrame>) -> u32`:
  picks the next task by min vruntime (CFS). Returns the task index.
  Does NOT load the context — just returns the index. The caller
  (arch-specific context switch) loads the saved TrapFrame.
- Write `fn sched_save_context(tbl: State<ProcessTable>, task_idx: u32, tf: State<TrapFrame>)`:
  copies the current TrapFrame's gpr/pc/status/sp into the task's saved
  context (stored in ProcessTable's flat arrays).
- Write `fn sched_restore_context(tbl: State<ProcessTable>, task_idx: u32, tf: State<TrapFrame>)`:
  copies the task's saved context back into the TrapFrame.
- `fn main() -> i32` self-test.

**DoD:**
- [ ] Scheduler uses ONLY superset TrapFrame (no arch-specific fields)
- [ ] `schedule`, `sched_save_context`, `sched_restore_context` verify
- [ ] Self-test exits 0
- [ ] No pointer syntax

### R4b — Arch context switch for superset TrapFrame

**Files:** `womb/kernel/arch/x86_64/switch.vuma` (update), `womb/kernel/arch/aarch64/switch.vuma` (update), `womb/kernel/arch/riscv64/switch.vuma` (update)

**Contract:**
- Each file re-declares superset TrapFrame + helpers.
- Write `fn arch_context_switch(prev_tf: State<TrapFrame>, next_tf: State<TrapFrame>)`:
  In hosted mode: no-op (the scheduler's `sched_save/restore_context` already
  copied the TrapFrame data). In bare-metal: the asm stub loads `next_tf`'s
  gpr/pc/status/sp into hardware registers and returns.
- Declare `extern "C" fn arch_load_context(tf: State<TrapFrame>)` (asm stub).
- `fn main() -> i32` self-test.

**DoD:**
- [ ] All 3 switch.vuma files compile + verify on x86_64
- [ ] `arch_context_switch` + `arch_load_context` verify
- [ ] No pointer syntax

### R4c — Unified syscall dispatcher

**Files:** `womb/kernel/syscall/dispatch.vuma` (rewrite)

**Contract:**
- Re-declare superset TrapFrame + helpers + SyscallTable + helpers.
- Write `fn syscall_dispatch_from_trap(tbl: State<SyscallTable>, tf: State<TrapFrame>) -> u64`:
  - Extract syscall number from `tf_get_gpr(tf, syscall_nr_slot)` where
    `syscall_nr_slot` is: x86_64=0 (rax→slot 0, but rax is slot 0 in
    canonical order — actually rax is gpr[0]), aarch64=x8=slot 8,
    riscv64=a7=slot 17.
    **Simplification:** use a single canonical slot (slot 0 = syscall nr,
    slot 1 = arg0, ..., slot 6 = arg5, slot 7 = return value). Each arch's
    `trap_to_superset` maps the hardware syscall register to slot 0.
  - Bounds-check nr < 512.
  - Look up handler in SyscallTable.
  - Return result in slot 7.
- Write `fn syscall_set_ret(tf: State<TrapFrame>, ret: u64)`:
  `tf_set_gpr(tf, 7, ret)`.
- `fn main() -> i32` self-test.

**DoD:**
- [ ] `syscall_dispatch_from_trap` uses ONLY superset TrapFrame
- [ ] Canonical slot mapping documented (slot 0=nr, 1-6=args, 7=ret)
- [ ] Self-test exits 0
- [ ] No pointer syntax

### R4d — Wasm32 cooperative scheduling

**Files:** `womb/kernel/arch/wasm32/sched_hal.vuma` (new)

**Contract:**
- Re-declare superset TrapFrame + Runqueue + ProcessTable.
- Write `fn wasm_sched_loop(rq: State<Runqueue>, tbl: State<ProcessTable>)`:
  cooperative scheduling loop. Calls `schedule(rq, tbl, tf)`, then
  `sched_restore_context(tbl, next, tf)`, then yields back to host
  via `extern "C" fn host_yield()`.
- Declare `extern "C" fn host_yield()`.
- Document that wasm32 uses cooperative scheduling (no preemption).
- `fn main() -> i32 { return 0; }`.

**DoD:**
- [ ] File compiles + verifies on x86_64 (cross-compile)
- [ ] `wasm_sched_loop` verifies
- [ ] Cooperative scheduling documented
- [ ] No pointer syntax

---

## Wave R5 — Bare-Metal Trampolines

**Goal:** Write real assembly trampolines for boot entry, IRQ entry,
and context switch. One small `.S` file per arch. The PMT core is
already arch-independent; these trampolines just fill the superset
TrapFrame and call the PMT dispatcher.

**Max parallel:** 5 (all parallel, different arch directories).

**NOTE:** This wave requires cross-assemblers (`aarch64-linux-gnu-as`,
`riscv64-linux-gnu-as`, etc.) and QEMU system emulators. If unavailable,
write the `.S` files and document that testing is deferred.

### R5a — x86_64 trampolines

**Files:** `womb/kernel/arch/x86_64/boot.S` (new), `womb/kernel/arch/x86_64/trap.S` (new), `womb/kernel/arch/x86_64/switch.S` (new), `womb/kernel/arch/x86_64/linker.ld` (new)

**Contract:**
- `boot.S`: multiboot2 header + `_start` (set sp, zero BSS, call `main`).
- `trap.S`: IDT vector table (256 entries), `trap_entry` saves all GPRs
  into the superset TrapFrame's `gpr` array (canonical order: rax=slot 0,
  rbx=slot 1, ..., r15=slot 14), sets pc/status/vector/sp, calls
  `trap_handler`, `trap_exit` restores + `iretq`.
- `switch.S`: `arch_load_context` loads gpr/pc/status/sp from TrapFrame, `ret`.
- `linker.ld`: entry=_start, .text@1MB, .bss@0x90000.

**DoD:**
- [ ] `boot.S` assembles with `as`
- [ ] `trap.S` assembles with `as`
- [ ] `switch.S` assembles with `as`
- [ ] `linker.ld` links with `ld`
- [ ] Canonical register order documented in comments

### R5b — aarch64 trampolines

**Files:** `womb/kernel/arch/aarch64/boot.S`, `trap.S`, `switch.S`, `linker.ld`

**Contract:**
- `boot.S`: QEMU virt entry @0x40080000, set sp, zero BSS, call `main`.
- `trap.S`: VBAR_EL1 vector table (16 entries), `trap_entry` saves x0-x30
  into gpr[0..30], saves sp_el0/elr_el1/spsr_el1/esr_el1, calls
  `trap_handler`, `trap_exit` restores + `eret`.
- `switch.S`: `arch_load_context` loads x0-x30/sp/pc from TrapFrame, `eret`.
- `linker.ld`: entry=_start, .text@0x40080000.

**DoD:**
- [ ] All 4 files written
- [ ] Assembles with `aarch64-linux-gnu-as` (if available)
- [ ] Canonical register order documented

### R5c — riscv64 trampolines

**Files:** `womb/kernel/arch/riscv64/boot.S`, `trap.S`, `switch.S`, `linker.ld`

**Contract:**
- `boot.S`: QEMU virt entry @0x80200000, set sp, zero BSS, call `main`.
- `trap.S`: `stvec` vector table, `trap_entry` saves x1-x31 into gpr[0..30]
  (canonical order), saves mepc/mstatus/scause/stval, calls `trap_handler`,
  `trap_exit` restores + `sret`.
- `switch.S`: `arch_load_context` loads regs from TrapFrame, `sret`.
- `linker.ld`: entry=_start, .text@0x80200000.

**DoD:**
- [ ] All 4 files written
- [ ] Assembles with `riscv64-linux-gnu-as` (if available)
- [ ] Canonical register order documented

### R5d — ppc64le trampolines

**Files:** `womb/kernel/arch/ppc64le/boot.S`, `trap.S`, `switch.S`, `linker.ld`

**Contract:**
- `boot.S`: QEMU pSeries entry, set sp, zero BSS, call `main`.
- `trap.S`: save r0-r31 into gpr[0..31], save pc/lr/msr/srr1, call
  `trap_handler`, restore + `rfid`.
- `switch.S`: `arch_load_context`, `rfid`.
- `linker.ld`: entry=_start.

**DoD:**
- [ ] All 4 files written
- [ ] Canonical register order documented

### R5e — Hosted-mode framework (for archs without bare-metal)

**Files:** `womb/kernel/hosted/host.vuma` (update), `womb/kernel/arch/hosted/boot.S` (new, minimal), `womb/kernel/arch/hosted/linker.ld` (new)

**Contract:**
- `host.vuma`: update to use superset BootInfo + EarlyConsole.
  `fn host_init(bi: State<BootInfo>)` sets boot_type=3 (hosted), fills
  mem_start/mem_size from host.
- `boot.S`: just `call main; mov eax, 60; syscall` (Linux process exit).
- `linker.ld`: standard Linux ELF linker script (or use host's default).

**DoD:**
- [ ] `host.vuma` uses superset BootInfo
- [ ] `boot.S` + `linker.ld` exist
- [ ] Hosted kernel still boots via `kernel_smoke.sh`

---

## Wave Dependency Graph

```
R1a ─┐
R1b ─┤
R1c ─┼──► R1e (integration)
R1d ─┘

R2a ─┐
R2b ─┤
R2c ─┼──► R2f (integration)
R2d ─┤
R2e ─┘

R3a ──────────────── (independent audit)
R3b ─┐
R3c ─┼──► R3d ∥ R3e ∥ R3f (arch-specific walkers)

R4a ∥ R4b ∥ R4c ∥ R4d  (all parallel)

R5a ∥ R5b ∥ R5c ∥ R5d ∥ R5e  (all parallel, needs R1-R4 complete)
```

---

## Subagent Protocol

Every subagent MUST:
1. Read `/home/z/my-project/worklog.md` before starting.
2. `cd /home/z/vuma` before every command.
3. Build: `. "$HOME/.cargo/env" && cargo build --profile release-fast --bin compile_dump`
4. Test: `./target/release-fast/compile_dump <file> /tmp/out.bin x86_64 --verify`
5. PMT-only: no `*ptr`, `&x`, `allocate`, `free`.
6. Touch ONLY the files listed in the task.
7. Commit per task: `git commit -m "Wave R<N><sub>: <title>"`.
8. APPEND a section to `/home/z/my-project/worklog.md`:
   ```
   ---
   Task ID: R<N><sub>
   Agent: <name>
   Task: <one-line>
   Work Log:
   - <step>
   Stage Summary:
   - <result>
   ```

---

## Global DoD

- [ ] Superset BootInfo defined + all archs populate it
- [ ] Superset TrapFrame defined + all archs populate it
- [ ] Universal EarlyConsole with single `write_byte` interface
- [ ] IRQ ring buffer decouples asm entry from PMT dispatch
- [ ] Trap handler is 100% arch-independent (uses superset TrapFrame only)
- [ ] Scheduler is 100% arch-independent
- [ ] Syscall dispatcher uses canonical TrapFrame slots (0=nr, 1-6=args, 7=ret)
- [ ] Generic VmmNode + arch-specific page-table walkers
- [ ] Wasm32 cooperative scheduling documented
- [ ] Bare-metal trampolines for x86_64, aarch64, riscv64, ppc64le
- [ ] Hosted-mode framework for remaining 14 archs
- [ ] Zero `#ifdef` or arch-conditional branches in core kernel transforms
- [ ] All 19 backends pass `scripts/kernel_parity.sh`
- [ ] `scripts/kernel_smoke.sh` passes

---

## Success Criteria

1. **One TrapFrame, all archs.** The same `trap_handler(tf: State<TrapFrame>)`
   compiles and runs correctly on x86_64, aarch64, riscv64, ppc64le, and
   wasm32 without any arch-conditional code.

2. **One BootInfo, all archs.** `kmain` reads the same fields regardless
   of whether the boot stub was Multiboot2, DTB, SBI, or host-injected.

3. **One EarlyConsole, all archs.** `early_console_write_byte` works on
   16550, PL011, semihosting, diag, and wasm host callbacks via a single
   `con_type` dispatch.

4. **Arch differences at the edge only.** The only arch-specific files
   are in `womb/kernel/arch/<isa>/` (HAL transforms + assembly trampolines).
   Everything in `womb/kernel/{mm,proc,vfs,trap,syscall,ipc,sync,...}/`
   is 100% arch-independent.

5. **Golden Rule enforced.** No `if arch == ...` inside any kernel
   transform outside `womb/kernel/arch/`.

*This spec is a living document. Update DoD checkboxes as waves complete.*
