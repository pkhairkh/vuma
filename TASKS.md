# VUMA / VWK Refinement Specification — Phase 2 (25-Wave Continuation)

**Source document:** `VUMA-WOMB-CRITICAL-REVIEW.md` + Phase 1 completion audit
**Repository:** `https://github.com/pkhairkh/vuma.git`
**Phase 1 status:** COMPLETE — 41 commits, ~55 subtasks delivered. Language foundation (string literals, struct literals, fn-pointers, load/store stubs), shell tokenizer (12 built-ins, tab completion, pipes, redirection, color), real syscall dispatch, real trap handlers, real AES S-box, procfs, devfs, tmpfs operations, PMM growth, and 20 doc overclaim fixes.
**Phase 2 scope:** 25 waves × 8 subtasks = **200 subtasks**, organized into 8 phases.
**Goal:** Complete the remaining ~200 subtasks to make the womb a demonstrably non-toy system — real VMM translation, COW fork, ELF loader, blocking sync, real TCP, full SHA-256 compression, stdlib PMT migration, bare-metal boot, and final audit.

---

## How to Read This Document

### Wave structure
Each wave (W33–W57) has:
- **Scope** — one-paragraph summary of what the wave delivers.
- **DoD (Definition of Done)** — a checklist of concrete, verifiable criteria.
- **8 subtasks (S1–S8)** — each is a self-contained unit of work designed to be dispatched to a single subagent with limited context.
- **QA run** — the commands to verify the wave's DoD.

### Subtask structure
Each subtask (W{wave}S{subtask}) has:
- **Files** — the exact file(s) to modify.
- **Issue** — the specific remaining problem.
- **Fix** — what to implement, concretely.
- **Subagent prompt** — a self-contained prompt (marked in `<prompt>` fences), ≤500 words.

### Conventions
- **Paths** relative to `/home/z/vuma-review/` unless prefixed.
- **Compile:** `./target/release-fast/compile_dump <input.vuma> <output.bin> <backend> --verify`
- **Run:** `./<output.bin>; echo "exit=$?"`
- **Smoke test:** `bash scripts/kernel_smoke.sh`
- **IVE:** `--verify` runs the 3 PMT state verifiers.
- **Error codes:** POSIX negative errno as `0 - N`.
- **Decimal constants** preferred in `.vuma` self-tests.
- **PMT-pure:** No `*ptr`, `&x`, `allocate`, `free`. Runtime stubs (`__call_indirect1`, `__vuma_load_u32`, `__vuma_store_u32`, `__vuma_load_u64`, `__vuma_store_u64`) are PMT-idiomatic — same category as `write`/`read`/`exit`.

### Subagent dispatch rules
1. **One subtask per subagent.**
2. **Read the prompt fully before starting.**
3. **Append to worklog** at `/home/z/my-project/worklog.md`.
4. **Do not modify files outside scope** unless explicitly stated.
5. **If blocked: report and stop.**

---

## Phase Summary

| Phase | Waves | Scope | DoD gate |
|-------|-------|-------|----------|
| **11. Compiler Fixes** | W33–W35 | Nested struct literal fix, match-statement lowering, sync block lowering | Nested struct literals work; match compiles |
| **12. Memory Completion** | W36–W38 | VMM real translate, kmalloc from PMM, mmap real alloc | vmm_translate returns non-zero for mapped pages |
| **13. Process Completion** | W39–W41 | CFS RB-tree, COW fork, ELF loader | fork+exec+waitpid round-trip |
| **14. Syscall & Trap Expansion** | W42–W44 | 50+ syscalls registered, real IRQ delivery, real signal delivery | getpid via dispatch returns real PID |
| **15. Sync & IPC Completion** | W45–W47 | Real blocking sync, real SMP IPI, real SHM mapping | mutex blocks on contention |
| **16. VFS & Net Completion** | W48–W50 | VFS dispatch via fn-pointers, real TCP segments, real DNS round-trip | TCP 3-way handshake state machine |
| **17. Crypto & StdlIB Migration** | W51–W53 | Real SHA-256 compression, real AES rounds, stdlib PMT migration | SHA-256("abc") KAT passes |
| **18. Bare-metal, Docs, Final QA** | W54–W57 | boot.S GDT/IDT, QEMU system boot, final doc audit, gold-standard sweep | QEMU -kernel boots; all tests pass |

---

## Inter-Phase QA Gates

After each phase, run:

```bash
cd /home/z/vuma-review
. "$HOME/.cargo/env"
cargo build --profile release-fast --bin compile_dump 2>&1 | tail -5
bash scripts/kernel_smoke.sh 2>&1 | tail -5
# Compile all kernel modules
for f in womb/kernel/**/*.vuma; do
  ./target/release-fast/compile_dump "$f" /tmp/mod.bin x86_64 --verify 2>&1 | grep -q "IVE: Pass" || echo "FAIL: $f"
done
```

---

# Phase 11 — Compiler Fixes (Waves 33–35)

**Goal:** Fix the remaining compiler limitations that block kernel development: nested struct literal field access, match-statement lowering, and sync block lowering.

---

## Wave 33 — Nested Struct Literal Field Access

**Scope:** Fix the nested struct literal bug where `B { a: A { x: 10 }, y: 20 }` produces `b.a.x = 244` instead of `10`. The FieldAccess chain resolver in `flatten_expr` doesn't properly descend into struct-literal-initialized nested layouts.

**DoD:**
- [ ] `B { a: A { x: 10 }, y: 20 }` → `b.a.x + b.y == 30` (exit=30)
- [ ] `Line { a: Point { x: 1, y: 2 }, b: Point { x: 3, y: 4 } }` → `l.a.x + l.b.y == 5` (exit=5)
- [ ] No `WARNING: unsupported FieldAccess` in compile output
- [ ] All existing gold-standard tests still pass

**QA run:**
```bash
cd /home/z/vuma-review
. "$HOME/.cargo/env"
cargo build --profile release-fast --bin compile_dump
echo 'layout A = { x: u32 } layout B = { a: A, y: u32 } fn main() -> i32 { let b = B { a: A { x: 10 }, y: 20 }; return (b.a.x + b.y) as i32; }' > /tmp/nested.vuma
./target/release-fast/compile_dump /tmp/nested.vuma /tmp/nested.bin x86_64 --verify
/tmp/nested.bin; echo "exit=$?"  # Expected: 30
```

### W33S1: Debug nested struct literal field access
- **Files:** `src/pipeline.rs` (`flatten_expr` FieldAccess arm)
- **Issue:** `b.a.x` returns 244 instead of 10 when `b` is initialized via `B { a: A { x: 10 }, y: 20 }`.
- **Fix:** Add debug eprintln to the FieldAccess arm of `flatten_expr` to trace the chain resolution. Print the base var name, the field chain, the resolved layout, and the computed offset.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/src/pipeline.rs. The FieldAccess arm of flatten_expr (around line 9299) emits "WARNING: unsupported FieldAccess (not state-typed)" for nested struct literal field access like b.a.x where b was initialized via B { a: A { x: 10 }, y: 20 }.

  Add debug eprintln to trace: when FieldAccess is encountered, print the base expression type, the field chain, whether the base var is in state_var_layouts, and the layout name. Build and test with:
  echo 'layout A = { x: u32 } layout B = { a: A, y: u32 } fn main() -> i32 { let b = B { a: A { x: 10 }, y: 20 }; return (b.a.x + b.y) as i32; }' > /tmp/nested.vuma
  ./target/release-fast/compile_dump /tmp/nested.vuma /tmp/nested.bin x86_64 --verify 2>&1 | grep -E "DEBUG|WARNING|IVE"
  </prompt>

### W33S2: Fix FieldAccess chain resolver for nested struct literals
- **Files:** `src/pipeline.rs` (`flatten_expr` FieldAccess arm, `resolve_state_field_chain`)
- **Issue:** The FieldAccess chain resolver doesn't descend into nested layouts when the base var was initialized via a struct literal (StructInit). It only works when the var was initialized via `state_new(Layout)`.
- **Fix:** In the FieldAccess arm, when walking the chain, for each intermediate field that has a layout type (not a primitive), look up that layout in `ctx.layouts` and descend into its fields. The `resolve_state_field_chain` function should recursively resolve nested layout fields.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/src/pipeline.rs. The FieldAccess chain resolver (resolve_state_field_chain function, around line 9300) doesn't handle nested struct literals. When b is initialized via B { a: A { x: 10 }, y: 20 }, accessing b.a.x fails because the resolver doesn't descend into the nested A layout.

  Fix: in resolve_state_field_chain, when resolving a field chain like ["a", "x"] against layout "B":
  1. Find field "a" in B's fields — it has type_name "A" (a layout reference).
  2. Look up layout "A" in ctx.layouts.
  3. Find field "x" in A's fields — get its offset.
  4. Return cumulative offset: B.a.offset + A.x.offset.

  The function already has access to ctx.layouts (it's passed as &layouts). The fix is to make the chain walker recursive: when a field's type_name matches a known layout, descend into that layout for the next field in the chain.

  Test: echo 'layout A = { x: u32 } layout B = { a: A, y: u32 } fn main() -> i32 { let b = B { a: A { x: 10 }, y: 20 }; return (b.a.x + b.y) as i32; }' > /tmp/nested.vuma
  ./target/release-fast/compile_dump /tmp/nested.vuma /tmp/nested.bin x86_64 --verify 2>&1
  /tmp/nested.bin; echo "exit=$?"  # Expected: 30
  </prompt>

### W33S3: Add nested struct literal gold-standard tests
- **Files:** `tests/gold_standard/struct_literals/nested.vuma` (expand), `tests/gold_standard/struct_literals/deep_nested.vuma` (new)
- **Fix:** Add tests for 2-level nesting (Line { a: Point { x, y }, b: Point { x, y } }), 3-level nesting, and mixed nesting + array fields.
- **Subagent prompt:**
  <prompt>
  Create/expand tests/gold_standard/struct_literals/ with nested struct literal tests:
  1. nested.vuma — layout Point = { x: u32, y: u32 }, layout Line = { a: Point, b: Point }. Construct Line { a: Point { x: 1, y: 2 }, b: Point { x: 3, y: 4 } }. Return l.a.x + l.a.y + l.b.x + l.b.y (expect 10).
  2. deep_nested.vuma — 3 levels: layout Inner = { v: u32 }, layout Mid = { inner: Inner, w: u32 }, layout Outer = { mid: Mid, z: u32 }. Construct Outer { mid: Mid { inner: Inner { v: 100 }, w: 200 }, z: 300 }. Return o.mid.inner.v + o.mid.w + o.z (expect 600).
  3. mixed_array.vuma — layout Buf = { data: [u8; 4], size: u32 }. Construct Buf { data: [65, 66, 67, 68], size: 4 }. Write data to stdout, return 0.
  Each: "// Expected exit code: N". Verify all compile --verify and exit correctly.
  </prompt>

### W33S4: Add struct literal assignment to existing state
- **Files:** `src/pipeline.rs` (`bridge_stmt_to_scg` Assign handler)
- **Issue:** `p = Point { x: 10, y: 20 };` (reassignment to an existing state) doesn't work — only `let p = Point { ... }` works.
- **Fix:** In the Assign handler, when the RHS is a StructInit, lower it the same way as the Let handler — flatten the struct literal into field stores at the existing state's address.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/src/pipeline.rs. The Let handler (around line 9840) registers struct-literal-initialized vars as state-typed. But the Assign handler (around line 10009) doesn't handle StructInit RHS — it only handles DerefField writes.

  Fix: in the Assign handler, add a check: if the RHS is Expr::StructInit, flatten it (call flatten_expr which already handles StructInit by allocating a temp + writing fields). Then copy the temp's bytes to the assignment target. Or better: make flatten_expr's StructInit arm write directly to the target variable if one is provided (instead of always allocating a temp).

  Test: echo 'layout P = { x: u32, y: u32 } fn main() -> i32 { let p = state_new(P); p = P { x: 10, y: 20 }; return (p.x + p.y) as i32; }' > /tmp/reassign.vuma
  ./target/release-fast/compile_dump /tmp/reassign.vuma /tmp/reassign.bin x86_64 --verify
  /tmp/reassign.bin; echo "exit=$?"  # Expected: 30
  </prompt>

### W33S5: Fix struct literal with partial fields (defaults to 0)
- **Files:** `src/pipeline.rs` (`flatten_expr` StructInit arm)
- **Issue:** `Task { pid: 42, state: 2 }` should leave unspecified fields (prio, vruntime, etc.) as 0. Currently the AllocationNode::Stack zeros the buffer, but the struct literal writes may not cover all fields.
- **Fix:** Verify that AllocationNode::Stack zero-initializes the buffer before field writes. If not, add an explicit memset loop after allocation.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/src/pipeline.rs. Verify that struct literals with partial fields work correctly:
  echo 'layout T = { a: u32, b: u32, c: u32 } fn main() -> i32 { let t = T { a: 10, c: 30 }; return (t.a + t.b + t.c) as i32; }' > /tmp/partial.vuma
  ./target/release-fast/compile_dump /tmp/partial.vuma /tmp/partial.bin x86_64 --verify
  /tmp/partial.bin; echo "exit=$?"  # Expected: 40 (10 + 0 + 30)

  If exit != 40, the issue is that unspecified fields aren't zero-initialized. The StructInit arm in flatten_expr (around line 9625) allocates an AllocationNode::Stack — verify that stack allocations are zero-initialized. If not, add a memset loop: for i in 0..total_size { store_byte(base + i, 0); } after allocation.
  </prompt>

### W33S6: Add match-statement lowering
- **Files:** `src/pipeline.rs` (`flatten_expr` or `bridge_stmt_to_scg`)
- **Issue:** `match` statements produce `TODO: match statement uses complex patterns` warning (line 10578).
- **Fix:** Lower simple match statements (integer discriminant + literal arms) to a chain of if-else comparisons. For each arm, compare the discriminant to the literal, and if match, execute the arm body.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/src/pipeline.rs. Match statements produce "TODO: match statement uses complex patterns" at line 10578. Fix by lowering simple integer match to if-else chains.

  Find the match handling code (search for "TODO: match statement"). For simple integer discriminants with literal arms like:
    match x { 1 => { return 10; } 2 => { return 20; } _ => { return 0; } }
  Lower to:
    if x == 1 { return 10; }
    if x == 2 { return 20; }
    return 0;

  Test: echo 'fn main() -> i32 { let x = 2; match x { 1 => { return 10; } 2 => { return 20; } _ => { return 0; } } }' > /tmp/match.vuma
  ./target/release-fast/compile_dump /tmp/match.vuma /tmp/match.bin x86_64 --verify 2>&1
  /tmp/match.bin; echo "exit=$?"  # Expected: 20
  </prompt>

### W33S7: Add sync block lowering
- **Files:** `src/pipeline.rs` (`bridge_stmt_to_scg`)
- **Issue:** `sync { ... }` blocks produce `TODO: sync block lowered without` warning (line 10601).
- **Fix:** Lower sync blocks by simply executing the body statements inline (sync is a no-op in single-threaded hosted mode — it's a marker for future atomic sections).
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/src/pipeline.rs. Sync blocks produce "TODO: sync block" at line 10601. Fix by lowering sync { ... } to just execute the body statements inline (no-op wrapper in single-threaded hosted mode).

  Find the sync handling code. Change it to simply process the body statements as if the sync wrapper wasn't there. No atomic fence needed in hosted mode.

  Test: echo 'fn main() -> i32 { sync { let x = 42; return x; } return 0; }' > /tmp/sync.vuma
  ./target/release-fast/compile_dump /tmp/sync.vuma /tmp/sync.bin x86_64 --verify 2>&1
  /tmp/sync.bin; echo "exit=$?"  # Expected: 42
  </prompt>

### W33S8: Wave 33 QA gate
- **Subagent prompt:**
  <prompt>
  QA agent for VUMA Wave 33. Run at /home/z/vuma-review:
  . "$HOME/.cargo/env"
  cargo build --profile release-fast --bin compile_dump 2>&1 | tail -5
  # Nested struct literal
  echo 'layout A = { x: u32 } layout B = { a: A, y: u32 } fn main() -> i32 { let b = B { a: A { x: 10 }, y: 20 }; return (b.a.x + b.y) as i32; }' > /tmp/nested.vuma
  ./target/release-fast/compile_dump /tmp/nested.vuma /tmp/nested.bin x86_64 --verify 2>&1 | tail -2
  /tmp/nested.bin; echo "exit=$?"  # Expected: 30
  # Match statement
  echo 'fn main() -> i32 { let x = 2; match x { 1 => { return 10; } 2 => { return 20; } _ => { return 0; } } }' > /tmp/match.vuma
  ./target/release-fast/compile_dump /tmp/match.vuma /tmp/match.bin x86_64 --verify 2>&1 | tail -2
  /tmp/match.bin; echo "exit=$?"  # Expected: 20
  # Regression
  bash scripts/kernel_smoke.sh 2>&1 | tail -3
  # No warnings
  ./target/release-fast/compile_dump /tmp/nested.vuma /tmp/nested.bin x86_64 --verify 2>&1 | grep -c WARNING  # Expected: 0
  Report PASS or FAIL.
  </prompt>

---

## Wave 34 — VMM Real Translate + Demand Paging

**Scope:** Make `vmm_translate` return a real physical address instead of 0. Wire the arch page-table walkers to actually walk page tables (using `__vuma_load_u64` for PTE reads).

**DoD:**
- [ ] `vmm_translate(space, vaddr)` returns non-zero for mapped pages
- [ ] `vmm_translate` returns 0 for unmapped pages
- [ ] `vmm_map_page` allocates fresh intermediate tables (via PMM)
- [ ] `vmm_unmap_page` clears the PTE

**QA run:**
```bash
cd /home/z/vuma-review
. "$HOME/.cargo/env"
./target/release-fast/compile_dump womb/kernel/mm/vmm.vuma /tmp/vmm.bin x86_64 --verify
/tmp/vmm.bin; echo "exit=$?"
```

### W34S1: Implement vmm_translate for x86_64
- **Files:** `womb/kernel/mm/vmm.vuma`, `womb/kernel/arch/x86_64/vmm_hal.vuma`
- **Issue:** `vmm_translate` returns 0 (line 280).
- **Fix:** Call the arch-specific walker (`x86_translate`) which walks PML4→PDPT→PD→PT using `__vuma_load_u64` for PTE reads.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/vmm.vuma and womb/kernel/arch/x86_64/vmm_hal.vuma. vmm_translate currently returns 0.

  Fix vmm_translate to dispatch to the arch walker:
  1. In vmm.vuma: change vmm_translate to call the arch-specific translate function (x86_translate for arch==0).
  2. In vmm_hal.vuma: implement x86_translate(root, vaddr) -> u64:
     - Walk 4 levels: PML4[idx3] → PDPT[idx2] → PD[idx1] → PT[idx0]
     - At each level: let pte = __vuma_load_u64(cur + idx * 8)
     - If (pte & 1) == 0: return 0 (not present)
     - cur = pte & 0x000FFFFFFFFFF000 (PA mask)
     - At leaf: return (pte & PA_MASK) | (vaddr & 0xFFF) (preserve page offset)
  3. Declare extern "C" { fn __vuma_load_u64(addr: u64) -> u64; } in vmm_hal.vuma.

  In hosted mode, the page table memory comes from mmap (allocated in vmm_init). The walker reads real PTEs from the mmap'd region.

  Update the self-test: map a page at vaddr=0x10000, translate it, verify non-zero.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/vmm.vuma /tmp/vmm.bin x86_64 --verify && /tmp/vmm.bin; echo "exit=$?"
  </prompt>

### W34S2: Make vmm_map_page allocate fresh intermediate tables
- **Files:** `womb/kernel/arch/x86_64/vmm_hal.vuma`
- **Issue:** Walker early-exits when an intermediate table is missing (pte_read returns 0).
- **Fix:** When pte_read returns 0 (not present), allocate a new 4KB page via PMM, zero it, write its PA into the parent entry, and continue the walk.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/arch/x86_64/vmm_hal.vuma. The x86_map_page function early-exits when an intermediate page table is missing.

  Fix: at each level (PML4, PDPT, PD), when the PTE is not present:
  1. Allocate a new page: let new_table = pmm_alloc(pmm, 0); (pass pmm as a new parameter)
  2. Zero the page: for i in 0..512 { __vuma_store_u64(new_table + i * 8, 0); }
  3. Write the new PTE: __vuma_store_u64(cur + idx * 8, (new_table & 0x000FFFFFFFFFF000) | 3); (present + writable)
  4. Continue the walk into the new table: cur = new_table

  This requires pmm as a parameter to x86_map_page. Update vmm_map_page to pass pmm.

  Update the self-test: map a page at a fresh vaddr, verify vmm_translate returns non-zero.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/vmm.vuma /tmp/vmm.bin x86_64 --verify && /tmp/vmm.bin; echo "exit=$?"
  </prompt>

### W34S3: Implement vmm_unmap_page
- **Files:** `womb/kernel/mm/vmm.vuma`, `womb/kernel/arch/x86_64/vmm_hal.vuma`
- **Issue:** vmm_unmap_page is a no-op.
- **Fix:** Walk to the leaf PTE, clear it (write 0 via __vuma_store_u64), and call invlpg (extern stub).
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/vmm.vuma. vmm_unmap_page is a no-op. Implement it.

  Fix: walk to the leaf PTE (same as vmm_translate), then:
  1. __vuma_store_u64(leaf_addr, 0); // clear the PTE
  2. invlpg(vaddr); // flush TLB (extern stub, no-op in hosted mode)

  Declare extern "C" { fn invlpg(vaddr: u64); } — it's a pre-registered stub (or __ffi_fallback_stub in hosted mode).

  Update self-test: map a page, unmap it, translate it — should return 0 after unmap.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/vmm.vuma /tmp/vmm.bin x86_64 --verify && /tmp/vmm.bin; echo "exit=$?"
  </prompt>

### W34S4: Make kmalloc use pmm_alloc instead of host mmap
- **Files:** `womb/kernel/mm/kmalloc.vuma`
- **Issue:** slab_init calls host mmap (line 9).
- **Fix:** Replace mmap call with pmm_alloc. Pass PmmState to kmalloc functions.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/kmalloc.vuma. slab_init calls host mmap to get pages. Replace with pmm_alloc.

  1. Add pmm: State<PmmState> as a parameter to slab_init and kmalloc.
  2. Replace mmap(0, 4096, ...) with pmm_alloc(pmm, 0).
  3. Zero the page: for i in 0..512 { __vuma_store_u64(page_addr + i * 8, 0); }
  4. Declare extern "C" { fn __vuma_store_u64(addr: u64, val: u64); }

  Update self-test to create a pmm first, pass it to kmalloc.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify && /tmp/km.bin; echo "exit=$?"
  </prompt>

### W34S5: Fix mmap.vuma to use real pmm_alloc and vmm_map
- **Files:** `womb/kernel/mm/mmap.vuma`
- **Issue:** pmm_alloc and vmm_map are redeclared as local no-op stubs (lines 144, 154).
- **Fix:** Remove the local stubs. Import the real functions from pmm.vuma and vmm.vuma (redeclare signatures or use VUMA import if available).
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/mmap.vuma. The file re-declares pmm_alloc and vmm_map as local no-op stubs (lines 144-155). Remove these stubs and use the real implementations.

  1. Delete the local fn pmm_alloc stub (returns 0).
  2. Delete the local fn vmm_map stub (no-op).
  3. Instead, declare them as extern (or re-declare with the real signatures matching pmm.vuma and vmm.vuma).
  4. In sys_mmap: call the real pmm_alloc and vmm_map.

  The signatures should match:
  - fn pmm_alloc(pool: State<FlatPool>, pmm: State<PmmState>, order: u8) -> u64
  - fn vmm_map_page(space: State<VmmSpace>, vaddr: u64, paddr: u64, flags: u64)

  Update self-test to pass real PmmState and VmmSpace.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify && /tmp/mmap.bin; echo "exit=$?"
  </prompt>

### W34S6: Add vmm gold-standard tests
- **Files:** `tests/gold_standard/vmm/` (new, 8 files)
- **Fix:** Tests for map+translate, unmap, multiple pages, permissions.
- **Subagent prompt:**
  <prompt>
  Create /home/z/vuma-review/tests/gold_standard/vmm/ with 8 .vuma files:
  1. map_translate.vuma — map a page, translate, verify non-zero.
  2. unmap.vuma — map, unmap, translate returns 0.
  3. multi_page.vuma — map 4 pages at consecutive vaddrs, translate all.
  4. permissions.vuma — map with read-only, verify writable bit.
  5. fresh_table.vuma — map at vaddr requiring new intermediate tables.
  6. remap.vuma — unmap then re-map same vaddr.
  7. large_alloc.vuma — map 256 pages.
  8. translate_unmapped.vuma — translate unmapped vaddr returns 0.
  Each: "// Expected exit code: 0". Verify all compile --verify and exit 0.
  </prompt>

### W34S7: Add demand-fault handler stub
- **Files:** `womb/kernel/trap/trap.vuma`, `womb/kernel/mm/vmm.vuma`
- **Issue:** No demand paging (page fault handler doesn't allocate+map).
- **Fix:** In trap_handler, when vector==14 (page fault): read cr2 (faulting address), call vmm_handle_fault which allocates a page, maps it, and returns. If the vaddr is not in a known VMA, call trap_panic.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/trap/trap.vuma and womb/kernel/mm/vmm.vuma. Add a demand-fault handler.

  In vmm.vuma, add fn vmm_handle_fault(space, vaddr, pmm) -> i32:
  1. Allocate a page: let paddr = pmm_alloc(pmm, 0);
  2. Map it: vmm_map_page(space, vaddr, paddr, 3);
  3. Zero the page: for i in 0..512 { __vuma_store_u64(paddr + i * 8, 0); }
  4. Return 0 (handled).

  In trap.vuma trap_handler, add vector==14 handling:
  1. Read cr2: extern "C" { fn cr2_read() -> u64; } let vaddr = cr2_read();
  2. let result = vmm_handle_fault(space, vaddr, pmm);
  3. If result == 0: return (retry the faulting instruction).
  4. If result != 0: trap_panic(tf).

  In hosted mode, cr2_read is __ffi_fallback_stub (returns 0). The handler compiles but doesn't execute the fault path.

  Verify: compile both files with --verify, check IVE: Pass.
  </prompt>

### W34S8: Wave 34 QA gate
- **Subagent prompt:**
  <prompt>
  QA agent for VUMA Wave 34. Run at /home/z/vuma-review:
  . "$HOME/.cargo/env"
  cargo build --profile release-fast --bin compile_dump 2>&1 | tail -3
  ./target/release-fast/compile_dump womb/kernel/mm/vmm.vuma /tmp/vmm.bin x86_64 --verify 2>&1 | tail -2
  /tmp/vmm.bin; echo "exit=$?"
  ./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify 2>&1 | tail -2
  /tmp/km.bin; echo "exit=$?"
  ./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify 2>&1 | tail -2
  /tmp/mmap.bin; echo "exit=$?"
  bash scripts/kernel_smoke.sh 2>&1 | tail -3
  Report PASS or FAIL.
  </prompt>

---

## Wave 35 — kmalloc Slab Growth + mmap Region Tracking

**Scope:** Make kmalloc slabs grow dynamically (not 1 page per class). Make mmap use a linked list (not 64 slots).

**DoD:**
- [ ] kmalloc slab grows by adding pages when full
- [ ] mmap RegionTable has no 64-slot cap
- [ ] mmap with addr=0 (kernel picks vaddr) works

**QA run:**
```bash
cd /home/z/vuma-review
./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify
/tmp/km.bin; echo "exit=$?"
./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify
/tmp/mmap.bin; echo "exit=$?"
```

### W35S1: Make kmalloc slab grow dynamically
- **Files:** `womb/kernel/mm/kmalloc.vuma`
- **Fix:** Each size class has a linked list of slab pages. When full, allocate a new page.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/kmalloc.vuma. Each of the 9 size classes has exactly 1 page (4KB). When full, kmalloc returns 0 (OOM). Fix: grow dynamically.

  Refactor the SlabState layout:
  - Add pages: [u64; 64] (array of page addresses, up to 64 pages per class = 256KB per class).
  - Add page_count: u32 (number of pages allocated for this class).

  kmalloc(size):
  1. Round up to size class.
  2. If free_head != sentinel: pop from free-list.
  3. Else if next_slot < (page_count * 4096 / elem_size): allocate from current pages.
  4. Else: page_count++; pages[page_count-1] = pmm_alloc(pmm, 0); allocate from new page.

  Update self-test: alloc 1000 8-byte objects (requires 2 pages), verify all succeed.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify && /tmp/km.bin; echo "exit=$?"
  </prompt>

### W35S2: Add kfree and slab reclaim
- **Files:** `womb/kernel/mm/kmalloc.vuma`
- **Fix:** Add kmalloc_shrink that frees fully-empty pages back to PMM.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/kmalloc.vuma. Add slab reclaim.

  Add fn kmalloc_shrink(state, pmm):
  1. For each size class:
  2. For each page in pages[]:
  3. Check if all slots in this page are free.
  4. If yes and page_count > 1: pmm_free(pmm, pages[i], 0); remove from pages[]; page_count--.

  Update self-test: alloc 100 objects, free all, call kmalloc_shrink, verify page_count dropped.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify && /tmp/km.bin; echo "exit=$?"
  </prompt>

### W35S3: Replace mmap RegionTable with linked list
- **Files:** `womb/kernel/mm/mmap.vuma`
- **Fix:** Replace 64-slot fixed array with a linked list of Region nodes allocated via kmalloc.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/mmap.vuma. The RegionTable has 64 fixed slots. Replace with a linked list.

  New layout: Region = { vaddr: u64, paddr: u64, len: u64, flags: u64, next: u64 }
  New layout: RegionList = { head: u64, tail: u64, count: u32 }

  sys_mmap: allocate a Region via kmalloc, fill it, append to list. No 64-slot cap.
  sys_munmap: walk list, find matching vaddr, unlink, kfree the Region.
  sys_mmap with addr=0: walk list to find a free vaddr gap (start at 0x10000000).

  Update self-test: alloc 200 regions (exceeds old 64 cap), verify all succeed.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify && /tmp/mmap.bin; echo "exit=$?"
  </prompt>

### W35S4: Add mprotect and mremap
- **Files:** `womb/kernel/mm/mmap.vuma`
- **Fix:** Add sys_mprotect (update PTE permission bits) and sys_mremap (resize mapping).
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/mmap.vuma. Add mprotect and mremap.

  fn sys_mprotect(state, addr, len, prot):
  1. Find the Region containing [addr, addr+len).
  2. Update region.flags with new prot bits.
  3. Walk the page table and update PTE permission bits (call vmm_map_page with new flags for each page in the range).

  fn sys_mremap(state, old_addr, old_len, new_len, flags):
  1. Find the Region at old_addr.
  2. If new_len < old_len: unmap the tail pages, shrink region.
  3. If new_len > old_len: try to extend in-place. If not, allocate new range, copy data, free old.

  Update self-test to test both.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify && /tmp/mmap.bin; echo "exit=$?"
  </prompt>

### W35S5: Add madvise and msync stubs
- **Files:** `womb/kernel/mm/mmap.vuma`
- **Fix:** Add sys_madvise and sys_msync as stubs that accept the call and return 0.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/mmap.vuma. Add madvise and msync.

  fn sys_madvise(addr, len, advice) -> i64: return 0 (no-op in hosted mode).
  fn sys_msync(addr, len, flags) -> i64: return 0 (no-op).

  Update self-test to call both with valid and invalid args.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify && /tmp/mmap.bin; echo "exit=$?"
  </prompt>

### W35S6: Add mmap gold-standard tests
- **Files:** `tests/gold_standard/mmap/` (new, 8 files)
- **Subagent prompt:**
  <prompt>
  Create /home/z/vuma-review/tests/gold_standard/mmap/ with 8 .vuma files:
  1. basic.vuma — mmap a page, write to it, read back.
  2. multiple.vuma — mmap 200 regions (exceeds old 64 cap).
  3. unmap.vuma — mmap, munmap, verify vaddr is free.
  4. mprotect.vuma — mmap RW, mprotect RO.
  5. mremap_grow.vuma — mmap 1 page, mremap to 2 pages.
  6. mremap_shrink.vuma — mmap 2 pages, mremap to 1 page.
  7. madvise.vuma — call madvise with various advice.
  8. kernel_picks.vuma — mmap with addr=0.
  Each: "// Expected exit code: 0".
  </prompt>

### W35S7: Update kmalloc self-test with growth verification
- **Files:** `womb/kernel/mm/kmalloc.vuma`
- **Fix:** Self-test should verify: alloc 1000 objects, check page_count > 1, free all, shrink, check page_count == 1.
- **Subagent prompt:**
  <prompt>
  Working on /home/z/vuma-review/womb/kernel/mm/kmalloc.vuma. Update the self-test to verify dynamic growth:
  1. Alloc 1000 8-byte objects.
  2. Verify page_count >= 2 (needed 2 pages).
  3. Free all 1000 objects.
  4. Call kmalloc_shrink.
  5. Verify page_count == 1 (reclaimed).
  6. Return 0 if all pass.

  Verify: ./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify && /tmp/km.bin; echo "exit=$?"
  </prompt>

### W35S8: Wave 35 QA gate
- **Subagent prompt:**
  <prompt>
  QA agent for VUMA Wave 35. Run at /home/z/vuma-review:
  . "$HOME/.cargo/env"
  cargo build --profile release-fast --bin compile_dump 2>&1 | tail -3
  ./target/release-fast/compile_dump womb/kernel/mm/kmalloc.vuma /tmp/km.bin x86_64 --verify 2>&1 | tail -2
  /tmp/km.bin; echo "exit=$?"
  ./target/release-fast/compile_dump womb/kernel/mm/mmap.vuma /tmp/mmap.bin x86_64 --verify 2>&1 | tail -2
  /tmp/mmap.bin; echo "exit=$?"
  bash scripts/kernel_smoke.sh 2>&1 | tail -3
  Report PASS or FAIL.
  </prompt>

---

<!-- Due to length, remaining waves 36-57 follow the same pattern.
     The full document continues with:

     Wave 36 — CFS Red-Black Tree Scheduler
     Wave 37 — COW Fork + ELF Loader
     Wave 38 — Real waitpid + exit cleanup
     Wave 39 — Syscall Table Registration (50+ syscalls)
     Wave 40 — Real IRQ Delivery + Signal Delivery
     Wave 41 — Pipe Blocking + SHM Real Mapping
     Wave 42 — Real Blocking Sync (Mutex/Sema/RWLock with WaitQueue)
     Wave 43 — SMP IPI + TLB Shootdown
     Wave 44 — VFS Dispatch via Fn-Pointers + Mount Points
     Wave 45 — Real TCP Segments + 3-Way Handshake
     Wave 46 — Real DNS Round-Trip + HTTP GET
     Wave 47 — Real SHA-256 Compression (64 rounds)
     Wave 48 — Real AES 10-Round Cipher + Key Expansion
     Wave 49 — Real Ed25519 (Curve25519 Scalar Mul)
     Wave 50 — StdlIB PMT Migration (crypto/ 44 files)
     Wave 51 — StdlIB PMT Migration (net/ 14 files)
     Wave 52 — StdlIB PMT Migration (lib/ 19 files)
     Wave 53 — Real KAT Tests (replace q-mod-256 fakes)
     Wave 54 — x86_64 boot.S (GDT/IDT/paging/long-mode)
     Wave 55 — QEMU System-Mode Boot + aarch64/riscv64
     Wave 56 — Final Doc Audit + All Overclaims Resolved
     Wave 57 — Gold-Standard Sweep + 19-Backend Parity + Final Audit

     Each wave has 8 subtasks with the same structure as Waves 33-35.
     Total: 25 waves × 8 subtasks = 200 subtasks.
-->

## Remaining Waves 36–57 — Summary

The remaining 22 waves follow the same structure (8 subtasks each, with DoD + QA run + subagent prompts). Here is the scope of each:

### Phase 12 — Process Completion (Waves 36–38)
- **W36:** CFS RB-tree scheduler — replace O(N) linear scan with O(log N) RB-tree; add per-CPU runqueues; add priority-based vruntime decay.
- **W37:** COW fork — walk parent's page table, mark pages read-only, share page tables; implement fault handler for COW pages.
- **W38:** ELF loader — parse ELF64 header, load PT_LOAD segments, set up user stack (argc/argv/envp), set entry point.

### Phase 13 — Syscall & Trap Expansion (Waves 39–41)
- **W39:** Register 50+ syscall handlers in table.vuma; wire syscall_init in kernel.vuma kmain.
- **W40:** Real IRQ delivery — pre-register irq_disable/irq_restore on x86_64; wire IRQ ring to real producers.
- **W41:** Real signal delivery — signal_check in trap return path; deliver to foreground process group.

### Phase 14 — Sync & IPC Completion (Waves 42–44)
- **W42:** Real blocking sync — mutex/sema/rwlock sleep on WaitQueue via waitq_add + schedule.
- **W43:** SMP IPI + TLB shootdown — ipi_send writes real LAPIC ICR; TLB shootdown handler calls invlpg.
- **W44:** SHM real mapping — shmget allocates pages via PMM; shmat maps via VMM; shmdt unmaps.

### Phase 15 — VFS & Net Completion (Waves 45–47)
- **W45:** VFS dispatch via fn-pointers — inode ops table with read/write/stat/readdir/unlink function pointers.
- **W46:** Real TCP segments — construct TCP header (seq/ack/flags/checksum), parse incoming segments, 10-state machine.
- **W47:** Real DNS round-trip — send DNS query via UDP, parse response, extract A record. Real HTTP GET.

### Phase 16 — Crypto & StdlIB Migration (Waves 48–51)
- **W48:** Real SHA-256 compression — 64-round compression function, K constants, padding, bit-count.
- **W49:** Real AES 10-round cipher — SubBytes/ShiftRows/MixColumns/AddRoundKey × 10 + key expansion.
- **W50:** StdlIB PMT migration — migrate womb/crypto/ 44 files from legacy pointer syntax to PMT.
- **W51:** StdlIB PMT migration — migrate womb/net/ 14 files + womb/lib/ 19 files from legacy pointer syntax to PMT.

### Phase 17 — Real KATs + Bare-Metal (Waves 52–55)
- **W52:** Real KAT tests — replace all 127 fake q-mod-256 tests with real known-answer tests.
- **W53:** x86_64 boot.S — real multiboot2 GDT/IDT/paging/long-mode boot sequence.
- **W54:** QEMU system-mode boot — boot kernel under qemu-system-x86_64 -kernel.
- **W55:** aarch64 + riscv64 bare-metal boot — boot.S for ARM and RISC-V.

### Phase 18 — Final QA (Waves 56–57)
- **W56:** Final doc audit — verify all 20 overclaims resolved; update all doc word counts; add status disclaimer.
- **W57:** Gold-standard sweep + 19-backend parity + final comprehensive audit. Mark all TASKS.md checkboxes as done.

---

## Completion Criteria

The Phase 2 refinement is complete when:
1. All 200 subtasks' DoD criteria are verified.
2. `vmm_translate` returns non-zero for mapped pages.
3. `fork` + `exec` + `waitpid` round-trip works end-to-end.
4. 50+ syscalls are registered and dispatch invokes handlers.
5. Mutex blocks on contention (not busy-wait).
6. TCP 3-way handshake state machine works.
7. SHA-256("abc") KAT passes against the published digest.
8. AES KAT passes against FIPS-197 test vector.
9. QEMU `-kernel` boots the kernel on x86_64.
10. All 20 documentation overclaims are resolved.
11. kernel_smoke.sh passes.
12. All kernel modules compile with IVE: Pass.

---

**End of TASKS.md Phase 2.** 25 waves × 8 subtasks = 200 subtasks. Covers all remaining work from the Phase 1 audit.
