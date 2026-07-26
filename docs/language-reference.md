# VUMA Language Reference

**Status:** Reference (IEEE-style). **Audience:** VUMA programmers.
**Scope:** lexical structure, types, expressions, statements, functions,
builtins, PMT, FFI, known caveats. **Cross-references:**
[Architecture Overview](../architecture/overview.md) ·
[Caveats](../caveats.md) · [Backends](../backends/).

This document describes what the **parser accepts**. Where the codegen does
not yet lower a construct, the entry links to
[../caveats.md](../caveats.md).

---

## 1. Lexical Structure

Source files are UTF-8. Comments: `// line` and `/* block (non-nesting) */`.
Identifiers: `[A-Za-z_][A-Za-z0-9_]*` (types PascalCase by convention).

**Keywords.** `fn let mut return if else while for loop break continue match
in as struct enum layout extern transform State spawn wait async await
unsafe borrow move ref true false None Some Ok Err i8 i16 i32 i64 u8 u16 u32
u64 f32 f64 bool Address Channel`.

`layout` and `transform` are reserved in **item position only**: at the top
of a module/file, `layout <Name> = { … }` and `transform <name>(…) -> … {
… }` dispatch to the layout/transform definition parsers. In every other
position (statements, expressions, `let` bindings, field accesses, etc.) they
are ordinary identifiers, so `let layout = 5;`, `layout.field = 7;`, and
`transform = 1;` all parse as regular statements. The dispatch is
peek-guarded lexeme dispatch in `parse_item` (`parser.rs:405-419`, mirroring
the `region` handling at `parser.rs:367-371`): the parser only routes to
`parse_layout_def`/`parse_transform_def` when the *next* token is an `Ident`
or a name-keyword (`Ok`/`Some`/`Err`/`ptr`/`alloc`/…). They are **not**
distinct `TokenKind` variants in the lexer (-a fix; see).

**Operators.** `+ - * / % & | ^ ! << >> && || == != < <= > >=
= += -= *= /= %= &= |= ^= <<= >>= as <- -> * & .::`

**Literals.** `42` (i32 default) · `42i64` · `0x1F_u8` · `0b1010_u16` ·
`0o755_u32` · `3.14` (f64 default) · `3.14f32` · `true`/`false` ·
`"utf-8\n\xHH\u{XXXX}"` · `b"bytes"`.

---

## 2. Types

### 2.1 Primitives

| Type | Width | Notes |
|-------------------------------|-------------:|----------------------------------------------------|
| `i8 i16 i32 i64` `u8 u16 u32 u64` | 8–64 bit | two's complement; signed `/` `%` are sdiv/srem |
| `f32 f64` | 32/64-bit IEEE-754 | soft-float on some backends |
| `bool` | 1 bit logical | stored as `u8`; `true == 1` |
| `Address` | 32 or 64-bit | raw pointer; width matches target pointer size |

### 2.2 Composite & concurrency

```vuma
struct Point { x: u32, y: u32 } // value type, declaration-order layout
enum Option { None, Some(u32) } // tagged union: [tag:u32@0][payload@4]
layout Point = { x: u32, y: u32 } // PMT state record (lives in arena)
struct Channel<T> { buffer: Address, cap: u64 } // generic struct
```

`Channel<T>`, `Sender<T>` (clonable), `Receiver<T>` (move-only), and
`AtomicU64` (with `Acquire`/`Release`/`Relaxed` orderings) are the
concurrency primitives.

---

## 3. Expressions

### 3.1 Arithmetic, bitwise, logical, comparison

```vuma
a + b a - b a * b a / b a % b // /% on i* are sdiv/srem
a & b a | b a ^ b !a a << n a >> n // bitwise (no floats — see caveat below)
a && b a || b !b // logical, short-circuit on bool
a == b a != b a < b a <= b a > b a >= b
```

Bitwise/shift/remainder ops on floats are rejected by `verify_float_op`
(`backend.rs:102`) **before any backend lowers them** — but only the AArch64
backend calls the verifier. On the other backends a buggy IR with `And`
on `f64` silently produces wrong code (see [../caveats.md](../caveats.md)).

**Max expression depth.** Expressions may nest up to **1024** levels deep
(`parser::MAX_EXPR_DEPTH`, `parser.rs:36`; raised from 256 in -b to
accommodate machine-generated bignum-KAT programs). Override via the global
`--max-expr-depth <N>` CLI flag (default `1024`; `0` is rejected). The limit
flows through `CompileConfig::max_expr_depth` into `Parser::with_max_depth`
at every parser entry point (`compile_with_path`, `compile_modules`,
`compile_to_wasm`, `cmd_emit`, `cmd_build_direct`, `cmd_run`, `cmd_compile`)
and into `ModuleResolver` for imported modules.

### 3.2 Cast, channel sugar, construction, access

```vuma
x as i64 ptr as Address i as f64 f as i32
ch <- value // sugar for channel_send(ch, value)
msg <- ch // sugar for channel_recv(ch)

p = Point { x: 1, y: 2 };
p.x // field read (value)
(*ptr).field // field read through Address (also ptr.field shorthand)
*(ptr + 8) = 42; // raw byte store through Address
Option::Some(42) // enum variant construction
```

---

## 4. Statements

```vuma
x = 7; x += 1; *p = 3; p.x = 1; // assignment (also -= *= /= %= &= |= ^= <<= >>=)

if cond { … } else if c2 { … } else { … }
while i < 10 { i += 1; }
for i in 0..10 { … }
loop { if done { break; } }
match value {
 Option::None => 0,
 Option::Some(v) => v + 1,
 _ => -1,
}
return expr; break; continue;
```

`match` desugars to a tag-compare cascade; variant payloads are bound by position.

---

## 5. Functions

```vuma
fn name(p1: T1, p2: T2) -> Ret { … return expr; }
fn no_return_type { … } // implicit unit return
fn id<T>(x: T) -> T { return x; } // generic
fn get_x(p: State<Point>) -> i32 { return p.x; } // state-typed parameter (PMT)
transform id(s: State<Point>) -> u32 { return s.x; } // parsed; lowering partial — see 
```

Scalars up to 16 bytes are passed by value; larger structs are passed by
reference through a hidden return slot. Multiple returns use a tuple desugared
to an aggregate slot. `extern "C"` follows the target C ABI.

```vuma
extern "C" {
 fn write(fd: i64, buf: Address, count: i64) -> i64;
 fn exit(code: i64);
}
```

The compiler emits relocations; the system linker resolves them at link time.

---

## 6. Builtins

Builtins expand to IR sequences in `src/codegen/src/ipc_lowering.rs`, shared
by all 19 backends.

```vuma
// Memory & arena
allocate(size: u64) -> Address free(ptr: Address)
arena_new(capacity: u64) -> Address
arena_alloc(arena: Address, layout: Layout) -> Address // traps to __arena_overflow on OOM
arena_destroy(arena: Address) // frees arena; invalidates all derived ptrs

// PMT state
state_new(Layout) -> State<Layout> state_read(s, field) -> U state_write(s, field, v)

// IPC & concurrency
channel_open(capacity: u64) -> Channel<T>
channel_send(ch, value) -> bool channel_recv(ch) -> Option<T>
channel_try_recv(ch) -> Option<T> channel_close(ch)
spawn_worker(fn, args...) -> WorkerId // lowers to Syscall{nr:220 (clone)}
wait_worker(id) -> i64

// Capability / sandbox (parsed; expansion coverage varies)
capability_grant(ch, cap) capability_delegate(cap, target)
sandbox_apply(policy) aead_seal(key, nonce, aad, pt) -> ct aead_open(...)
checkpoint_save(state) -> id checkpoint_restore(id)
circuit_breaker_open(name) hot_swap_replace(symbol, new_fn)

// Verification
stark_prove(program, witness) -> Proof stark_verify(program, proof) -> bool
formal_verify(program) -> VerificationReport
```

On `wasm32` only, `channel_*` builtins stay as `IRInstr::Call` (handled by the
backend's ring-buffer routines) and `spawn_worker` is emulated in-process via
`wasm32_fork_emulation_pass` — both parent and child branches run sequentially
in the same process. **This is not real isolation.**

The arena operations (`arena_new`, `arena_alloc`, `arena_destroy`) and the
PMT state ops (`state_new`, `state_read`, `state_write`) are mechanically
modeled and verified in Lean 4 — see [. Formal Verification](#10-formal-verification-lean-4)
and `proof/PMT/RawArena.lean` / `proof/PMT/PmtInstr.lean`.

---

## 7. PMT Model — Programs as Memory Transformations

**PMT in VUMA = "Programs as Memory Transformations,"** not "Persistent Memory
Transaction." There is no transaction, rollback, or durability machinery. PMT
is a verification discipline:

1. Every program is a state-transformation on a single backing arena
 (`___pmt_buffer`, an `mmap`'d region).
2. State lives in typed layouts (`PmtLayoutSpec`) registered with the
 verifier.
3. `arena_alloc` returns a state-typed pointer into that buffer.
4. Three state verifiers (state-read, state-write, state-transform) run by
 default at `VerificationLevel::Pmt`.
5. The five legacy pointer invariants (liveness, exclusivity,
 interpretation, origin, cleanup) are skipped at the default level —
 available at `VerificationLevel::Normal`.

Example: see `tests/gold_standard/pmt_wave2/state_param.vuma` (state-typed
parameter, field read in callee).

The default PMT level is mandatory since VUMA 2.0: `--no-memory-safety` is
removed and `CompileConfig.memory_safety=false` is silently ignored by
`pipeline.rs`.

The PMT memory model — arena allocation, state reads/writes, and state
transforms — is mechanically verified in Lean 4; the `pmt_soundness` and
`no_oob_trap_for_well_typed_strong` theorems hold for all well-typed programs.
See [. Formal Verification](#10-formal-verification-lean-4) and
[`../architecture/pmt-formal-spec.md`](../architecture/pmt-formal-spec.md).

**SCG lowering.** PMT constructs are no longer
opaque stubs on the Structured Call Graph:

| Construct | Lowering |
|-----------|----------|
| `layout <Name> = { f: T, … }` | `NodeType::StructDef` node carrying `StructFieldInfo { name, ty, offset, size }` (offsets pre-computed via `register_layout` in `to_scg.rs:174-195`, reusing the converter's alignment-padded table). (`to_scg.rs:644+`) |
| `transform <name>(…) -> … { … }` | Synthesized `FnDef` lowered via `convert_fn_def`; the `Item::TransformDef` arm at `to_scg.rs:683` reuses the function-def lowering. |
| `transform_call dst = name(args)` (statement form) | `ComputationNode` + `emit_call_nodes` (`FunctionEntry`/`FunctionReturn` pair + per-arg DataFlow edges + a `let`-binding for `dst`), reusing the `Expr::Call` lowering at `to_scg.rs:2288`. (`to_scg.rs:2276+`) |
| `state_new` / `state_read` / `state_write` / `state_transform` (NodePayload form) | `Call` to extern helpers `__vuma_state_init__<L>` / `__vuma_state_read__<L>__<f>` / `__vuma_state_write__<L>__<f>` / `__vuma_state_transform__<in>_to_<out>` + `ScgStatement::ForeignConsume` marker for linearity. (`pipeline.rs:2520+`,) |

In the production AST→SCG path (`to_scg.rs:992`) and the AST→codegen bridge
(`pipeline.rs:10370+`), `Expr::StateInit` / `ArenaNew` / `ArenaAlloc` /
`ArenaGrow` / `ArenaFree` are lowered directly to `Allocation`/`Access`/
`Call` nodes — so the `NodePayload` state arms above fire only for
IVE-test-constructed or deserialized SCGs (`scg/src/serialize.rs:1377+`).

---

## 8. FFI

```vuma
extern "C" {
 fn write(fd: i64, buf: Address, count: i64) -> i64;
 fn exit(code: i64);
}

#[borrow]
fn inspect(buf: Address, len: u64) -> u64 {
 // IVE tracks the borrow region; rejects aliased mutable access during it.
 return *buf;
}
```

The compiler emits relocations for `extern` symbols; the system linker (`ld`)
resolves them. Compile as a relocatable object and link with libc:
`vuma compile --format obj --target aarch64 prog.vuma -o prog.o && ld -o prog prog.o -lc`.
The borrow checker (`src/ive/src/borrow_region.rs`) tracks `#[borrow]` regions
and rejects conflicting `mut` access for the duration of the call. Per-backend
ABI tables are in [../backends/](../backends/).

### 8.1 Attributes

| Attribute | Meaning |
|-----------|---------|
| `#[borrow]` | Marks a function as a borrow region; IVE (`src/ive/src/borrow_region.rs`) rejects aliased `mut` access for the duration of the call. |
| `#[secret]` | Marks a variable/parameter as a **secret** for constant-time (CT) analysis. Collected by `collect_secret_vars` (`pipeline.rs:9043`) and attached to `VerificationInput::secret_vars`. When the program contains at least one `#[secret]` annotation, the IVE CT verifier consults the set exclusively via `VerificationInput::is_secret_value` (`verification.rs:45-63`) — sound, attribute-based detection. When the program has **no** `#[secret]` annotations, the verifier falls back to the legacy substring heuristic (`name.contains("secret")`) and emits a `vuma_log!(warn, …)` deprecation notice; this fallback is preserved for backwards-compat with older test programs and will be removed once all programs migrate. **`#[secret]` is the preferred way to mark secrets for CT analysis** (-e). |

---

## 9. Caveats — Parser Accepts, Codegen Does Not Lower

The VUMA parser accepts a strict superset of what the codegen lowers. The
full list is in [../caveats.md](../caveats.md);
the most impactful items:

1. **`transform` keyword** — parsed and **fully lowered to SCG**. The `transform_call` statement form and
 `Item::TransformDef` both lower via `emit_call_nodes` + `convert_fn_def`.
 The legacy `fn`-with-`State<T>` workaround
 (`tests/gold_standard/pmt_wave2/transform_id.vuma`) still works but is no
 longer required.
2. **`spawn_worker` on wasm32** — emulated in-process; both branches run
 sequentially with no memory protection between them.
3. **Bitwise/shift ops on floats** — accepted by parser, rejected by
 `verify_float_op` only on AArch64; silently wrong on the other 18.
4. **`--safe` CLI flag** — **MANDATORY** (`main.rs:607`
 hard-codes `safe: true`; `runtime_bounds_checks: cli.safe` at
 `main.rs:1563`). The flag is accepted for backwards-compat but cannot be
 disabled. `--no-memory-safety` is rejected (`main.rs:752-754`).
5. **Out-of-bounds array indexing through derived pointers** — not
 instrumented; only `arena_alloc` capacity overflow is trapped.
6. **`syscall_abi::translate`** — dead code; raw `nr` is emitted per backend,
 so `nr=1` is `write` on x86_64 but `io_destroy` on aarch64.
7. **HPPA backend** — Scaffolded tier; `Mul`/`Div`/`Cmp`/cond-branches emit
 stub code; requires QEMU LDIL workaround.
8. **`state_merge_compatible_layouts`** — stub returning `None`
 (`src/ive/src/bv_verify.rs:69`); deferred pending lifetime analysis.

---

## 10. Formal Verification (Lean 4)

The PMT memory model — arena allocation, state reads/writes, and state
transforms — is mechanically verified in **Lean 4** under `proof/`. The
library spans **20+ modules** totalling **~90 theorems** with only
**2 `sorry`s** (audited by `make proof-check`). Scope now covers the
operational semantics, the Iris invariant bundle
`[cap_bnd] ∗ [live_mirror] ∗ [guard]`, the `BitVecArena` overflow model,
the `MmapArena` allocator-failure model, and the `PipelineSim`
Lean↔Rust simulation. The proof builds with the Lake package manager and
is invoked by CI on every push.

### 10.1 Verified Components

| Surface operation | Lean module | Notes |
|----------------------------------------------------------|--------------------------------------------------|--------------------------------------------------------------------|
| `arena_new` / `arena_alloc` / `arena_grow` / `arena_free` | `proof/PMT/RawArena.lean` | RawArena phase machine; capacity-preservation invariant |
| `state.field = value` (write) | `proof/PMT/PmtInstr.lean` (`PmtInstr.store`) | Mirrors Rust `IRInstr::Store` |
| `state.field` (read) | `proof/PMT/PmtInstr.lean` (`PmtInstr.load`) | Mirrors Rust `IRInstr::Load` |
| `state_transform` (operational model) | `proof/PMT/Soundness.lean` | Stepping relation used by `pmt_soundness` |
| `state_transform` (soundness proof) | `proof/PMT/IVE/Soundness/Transform.lean` | Transform preserves well-typedness and the arena invariant |
| Iris invariant `[cap_bnd]` (capacity bound) | `proof/PMT/Iris/CapBndInvariant.lean` | `a.used ≤ a.capacity` with ghost resources `γ_used`, `γ_cap` |
| Iris invariant `[live_mirror]` (liveness mirror) | `proof/PMT/Iris/LiveMirrorInvariant.lean` | `own(γ_live, Ex live)` mirrors the Rust `live` set |
| Iris invariant `[guard]` (guard page) | `proof/PMT/Iris/GuardInvariant.lean` | Guard page `PROT_NONE` at `a.base + a.capacity`, witnessed by ghost |
| Full invariant bundle & composition (no `sorry`) | `proof/PMT/Iris/Composition.lean` | `alloc_preserves_pmt_invariants` over `[cap_bnd] ∗ [live_mirror] ∗ [guard]` |
| `BitVecArena` (usize overflow model) | `proof/PMT/BitVecArena.lean` | Models fixed-width `usize` wraparound — more faithful than Lean `Nat` |
| `MmapArena` (allocator-failure model) | `proof/PMT/MmapArena.lean` | Models `mmap` failure / `MAP_FAILED` paths omitted by the `Nat` model |
| `PipelineSim` (Lean↔Rust simulation) | `proof/PMT/PipelineSim.lean` | Relates Lean `Program` to the lowered Rust SCG IR (SimRel) |

### 10.2 Key Theorems

- **`pmt_soundness`** (`proof/PMT/Soundness.lean`): for any `PmtProgram p`
 and well-formed initial arena `s`, if `p` is well-typed then `exec p s`
 either produces a result value or traps with a **canonical exit code** —
 `1` (general trap), `134` (out-of-bounds/OOB), or `135`
 (use-after-free/UAF). No "stuck" state is reachable from a well-typed
 program.

- **`no_oob_trap_for_well_typed_strong`** (`proof/PMT/WellTypedStrong.lean`):
 for any program that is *well-typed-strong* (well-typed **and** initialized
 against a layout-satisfying initial arena), execution never traps with
 `.oob` (exit `134`). This is the formal underpinning of the PMT guarantee
 that well-typed state reads/writes stay in-bounds.

### 10.3 Build & Reproduce

```bash
make proof # top-level: invokes lake build under proof/
cd proof && lake build # direct: builds all PMT/ and IVE/ theories
make proof-check # sorry-free audit via scripts/check-lean.sh (2 sorries remain)
make proof-test # run the Lean test harness (proof/PMT/Test/*.lean)
make verify-all # Lean + CI + docs in one shot
```

CI runs `lake build` on every push — see `.github/workflows/proof-verify.yml`.
The full formal specification (Arena, Layout, Allocation, State Value,
trap-code taxonomy, theorem statements, and proof strategy) is in
[`../architecture/pmt-formal-spec.md`](../architecture/pmt-formal-spec.md).

**In-tree verified checkers (`pmt-runtime-check` feature).** The Lean-
verified PMT checkers from `proof/PMT/Extraction.lean` are hand-translated
into Rust at `src/codegen/src/runtime/pmt_check.rs` (no longer living only
under `proof/extracted/`). The translation is *verified by parity test*
(`tests/pmt_parity_test.rs`, of testing overview) rather than by FFI
extraction. Enable the `pmt-runtime-check` Cargo feature on `vuma-codegen`
to swap the hand-written checkers in `arena.rs` for the verified set:
`cargo build -p vuma-codegen --features pmt-runtime-check`. When the feature
is off (default), the unverified hand-written checkers remain in effect.

### 10.4 What Is — and Is Not — Proven

- **Proven:** arena capacity preservation (`WF_RawArena`), `WF_Layout`
 closure under typed state ops, `no_oob_trap_for_well_typed_strong`, and
 `pmt_soundness` against the Lean operational semantics; the Iris
 invariant bundle `[cap_bnd] ∗ [live_mirror] ∗ [guard]` is preserved by
 `alloc` (`alloc_preserves_pmt_invariants`, `proof/PMT/Iris/Composition.lean`,
 no `sorry`); and `PipelineSim` (`proof/PMT/PipelineSim.lean`) provides
 the Lean↔Rust simulation refinement that was previously a roadmap item.
- **More faithful than `Nat` (BitVecArena):** the Lean operational
 semantics use `Nat` for sizes, which silently cannot overflow.
 `proof/PMT/BitVecArena.lean` re-develops the arena model over
 fixed-width bitvectors, capturing `usize` wraparound. The Rust parity
 translation (`pmt_check.rs::verified_capacity_check`) follows the
 `BitVec` model — `u64::checked_add` returning `None` on overflow — and
 is therefore *more* faithful to actual runtime behaviour than the bare
 Lean `Nat` model.
- **Allocator failure (MmapArena):** `proof/PMT/MmapArena.lean` models the
 `mmap` failure path (`MAP_FAILED`) that the idealised `Nat` arena omits.
- **Not proven (out of scope for this proof):** backends other than the
 reference IR interpreter (no per-backend machine-code soundness proof —
 see [ of `../backends/matrix.md`](../backends/matrix.md)).
