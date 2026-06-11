# VUMA Compiler: 91% → 100% Production Readiness Plan

## Wave-Based Execution with Maximum Parallelism

**Current State**: 91/100 weighted score, 2,591 tests passing, zero compilation errors, 6 clippy warnings, 10 TODO comments.

**Target State**: 100/100 — zero warnings, zero TODOs, every module at its most sophisticated form, all spec requirements met (ARM64 no regress, x86_64 native exec, QEMU exec, Wasm wasmtime validate).

**Execution Rules**:
- Each wave contains independent tasks that can all run in parallel
- Waves are ordered by dependency — later waves depend on earlier ones
- Every task gets a fully-specified subagent prompt
- Verification gate after every wave: `cargo check --workspace` zero errors, `cargo test --workspace` zero failures, `cargo clippy --workspace` zero warnings, git commit + push
- Maximum 256 subagents per wave (far more than we need; actual counts are 4–20 per wave)

---

## WAVE 0: Trivial Fixes (Zero-Cost Wins)

**Goal**: Eliminate all clippy warnings, dead code, and TODO comments. This unblocks the zero-warning policy for all subsequent waves.

**Subagents**: 8 (all independent)

### W0-T1: Fix 3 Dead-Code Warnings in Disassemblers

```
Task ID: W0-T1
Agent: full-stack-developer

You are fixing 3 dead-code warnings in the VUMA compiler codegen crate.

File 1: /home/z/my-project/vuma/src/codegen/src/arm32/disasm.rs
- Line 95: `fn sign_extend_12(val: u32) -> i32` is never used
- Fix: Either prefix with `#[allow(dead_code)]` if it may be needed for future ARM32 branch decoding, OR find a call site in arm32/mod.rs where it should be used and use it. Check if ARM32 branch offset decoding needs sign_extend_12 — if so, wire it in. If not, add `#[allow(dead_code)]` with a comment explaining why.

File 2: /home/z/my-project/vuma/src/codegen/src/loongarch64/disasm.rs
- Line 81: `fn fpr_from_bits(bits: u32) -> Fpr` is never used
- Fix: Either wire it into LoongArch64 floating-point instruction decoding (check loongarch64/mod.rs for FPR references), or add `#[allow(dead_code)]`.

- Line 135: `fn sign_extend_20(val: u32) -> i32` is never used
- Fix: Either wire it into LoongArch64 branch offset decoding, or add `#[allow(dead_code)]`.

After fixing, run: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo clippy -p vuma-codegen 2>&1 | grep "dead_code"
Verify: zero dead_code warnings remain.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T1.
```

### W0-T2: Fix 3 Collapsible-Match Warnings in x86_64 Disassembler

```
Task ID: W0-T2
Agent: full-stack-developer

You are fixing 3 "collapsible if" clippy warnings in the VUMA x86_64 disassembler.

File: /home/z/my-project/vuma/src/codegen/src/x86_64/disasm.rs

The warnings occur in the `decode_immediate` function (or similar) where nested if-else
blocks inside a match arm can be collapsed. Clippy says "this `if` can be collapsed
into the outer `match`".

Fix all 3 by restructuring the match arms to use pattern guards instead of nested if
blocks. For example, change:
    match disp_size {
        1 => {
            if adv < bytes.len() { ... } else { 0 }
        }
        2 => {
            if adv + 4 <= bytes.len() { ... } else { 0 }
        }
    }
To:
    match disp_size {
        1 if adv < bytes.len() => { ... }
        1 => 0,
        2 if adv + 4 <= bytes.len() => { ... }
        2 => 0,
    }

After fixing, run: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo clippy -p vuma-codegen 2>&1 | grep "collapsible"
Verify: zero collapsible_match warnings remain.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T2.
```

### W0-T3: Resolve 10 TODO Comments

```
Task ID: W0-T3
Agent: full-stack-developer

You are resolving all 10 TODO/FIXME comments in the VUMA compiler workspace at /home/z/my-project/vuma/

The TODOs are:
1. src/ive/src/constraint.rs:153 - "TODO: Implement actual constraint checking against SCG / model state."
2. src/ive/src/constraint.rs:166 - "TODO: Implement proper negation per constraint kind."
3. src/tests/src/framework.rs:802 - "TODO: Wire through vuma-codegen once the crate compiles."
4. src/projection/src/conversational.rs:1297 - "TODO: Replace with LLM-backed suggestion engine."
5. src/projection/src/textual.rs:286 - "TODO: Allow user-defined formatting templates."
6. src/projection/src/textual.rs:395 - "TODO: Custom template for capability display."
7. src/cor/src/runtime.rs:277 - "TODO: Look up which regions each edge belongs to and"
8. src/cor/src/runtime.rs:340 - "TODO: Pass actual per-region edge observations and contention"
9. src/codegen/src/emit.rs:2063 - "CMP instruction emission (CSET TODO)."
10. src/codegen/src/loongarch64/mod.rs:2138 - "TODO: actual clo.d when Instruction enum gains it"

For each TODO:
- If the feature is already implemented but the TODO was left behind: remove the TODO comment.
- If the feature is NOT implemented: IMPLEMENT IT. Do not just remove the comment.
  - #1-2: Implement real constraint checking and negation (check what ConstraintKind variants exist and implement match arms)
  - #3: Wire vuma-codegen into the test framework (the crate compiles now)
  - #4: Implement an LLM-backed suggestion engine stub that uses z-ai-web-dev-sdk (backend only)
  - #5-6: Implement user-defined formatting templates with a TemplateEngine struct
  - #7-8: Look up region edge mappings from the SCG and pass real observations
  - #9: Implement CSET emission for AArch64 (CSINC rd, XZR, XZR, invert(cc))
  - #10: Add clo.d instruction to LoongArch64 Instruction enum and wire into ISel

After fixing, run: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && rg "TODO|FIXME" --type rust src/
Verify: zero TODO/FIXME comments remain.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T3.
```

### W0-T4: Fix `let x;` Uninitialized Placeholder

```
Task ID: W0-T4
Agent: full-stack-developer

You are fixing the `let x;` uninitialized variable handling in the VUMA parser.

File: /home/z/my-project/vuma/src/parser/src/parser.rs
Current code (lines ~922-931):
    let value = if self.at(TokenKind::Assign) {
        self.advance();
        self.parse_expr()?
    } else {
        // `let x;` without initializer — use a placeholder
        Expr::Lit {
            value: Lit::Bool(false),   // WRONG: silently replaces uninit with `false`
            span: Span::synthetic(),
        }
    };

Step 1: Add an `Uninitialized` variant to the `Expr` enum in:
   /home/z/my-project/vuma/src/parser/src/ast.rs
   ```rust
   /// An uninitialized binding (`let x;`).
   Uninitialized { span: Span },
   ```

Step 2: Change the parser to use Expr::Uninitialized instead of Expr::Lit:
   ```rust
   } else {
       Expr::Uninitialized { span: Span::new(self.previous.span.end, self.previous.span.end) }
   };
   ```

Step 3: Update the `to_scg.rs` AST-to-SCG converter to handle `Expr::Uninitialized`:
   - In /home/z/my-project/vuma/src/parser/src/to_scg.rs, find the match on Expr variants
   - Add a case for `Expr::Uninitialized` that creates a zero-value node or marks the variable as uninitialized

Step 4: Update any other files that match on Expr variants — search for `Expr::Lit` or `match expr` or `match self` in the parser crate and add the Uninitialized case.

Step 5: Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-parser
Verify: all parser tests still pass.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T4.
```

### W0-T5: Make `loop` a Reserved Keyword

```
Task ID: W0-T5
Agent: full-stack-developer

You are making `loop` a reserved keyword in the VUMA lexer/parser instead of a soft keyword matched by string comparison.

Files to modify:
1. /home/z/my-project/vuma/src/parser/src/lexer.rs — Add `TokenKind::Loop` to the keyword list (search for where `TokenKind::Break`, `TokenKind::Continue` are defined and add `Loop` similarly)
2. /home/z/my-project/vuma/src/parser/src/parser.rs — Change line ~887 from string-matching `lexeme == "loop"` to `TokenKind::Loop` matching, like how Break/Continue are handled on lines ~881-882
3. /home/z/my-project/vuma/src/parser/src/ast.rs — Verify Stmt::Loop doesn't need changes
4. Any file that matches on TokenKind variants — add Loop case

Also add `TokenKind::Unsafe` handling while you're at it:
- The lexer already has `TokenKind::Unsafe` but the parser doesn't produce an `UnsafeBlock` AST node
- Add `Expr::UnsafeBlock { inner: Box<Expr>, span: Span }` to ast.rs
- In parser.rs, when you encounter `TokenKind::Unsafe`, parse the following block/expression and wrap it in UnsafeBlock

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-parser
Verify: all parser tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T5.
```

### W0-T6: Fix x86_64 ISel Gaps (Free = NOP, Branch = placeholder offset)

```
Task ID: W0-T6
Agent: full-stack-developer

You are fixing known ISel gaps in the x86_64 backend where certain operations emit placeholders instead of real code.

File: /home/z/my-project/vuma/src/codegen/src/x86_64/mod.rs

Known gaps:
1. Line ~1778: `IRInstr::Free { ptr: _ }` emits `encode_nop()` — "Free is lowered to a runtime call; emit NOP for now"
   Fix: Implement a proper stack-free operation. If the allocation was on the stack (frame-based), Free is a no-op (stack is freed on function return). If it was heap-allocated, emit a call to a free() runtime function. Add a `encode_call_rel32` to a `__vuma_free` symbol with the pointer in RDI (SystemV ABI).

2. Line ~1860: `IRInstr::Branch { target: _ }` emits `encode_jmp_rel32(0)` — placeholder offset
   Fix: This is correct for now (offsets are patched during linking), but add a comment explaining this and ensure the ELF emission handles R_X86_64_PLT32/R_X86_64_PC32 relocations for branch fixups (this will be done in Wave 1 relocation work).

3. Line ~1870: `IRInstr::CondBranch { cond, true_target: _, false_target: _ }` emits placeholder offsets
   Fix: Same as above — add comments, ensure relocation infrastructure exists.

4. Line ~1763: `IRInstr::GetAddress { dst, name: _ }` emits `encode_mov_reg_imm64(d, 0)` — placeholder
   Fix: Add comment that this needs R_X86_64_64 relocation at link time.

After fixing, run: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-codegen
Verify: all codegen tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T6.
```

### W0-T7: Enable Disabled Module in vuma-core

```
Task ID: W0-T7
Agent: full-stack-developer

You are re-enabling a disabled module in the VUMA core driver.

File: /home/z/my-project/vuma/src/vuma/src/lib.rs
There is a commented-out line: `// pub mod n; // compile errors from other agent`

Step 1: Read the full lib.rs to understand the module structure
Step 2: Search for any module that was disabled — look for comments like "compile errors", "disabled", "commented out"
Step 3: If there's a source file for the disabled module, read it and fix any compilation errors
Step 4: Re-enable the module by uncommenting the `pub mod` line
Step 5: If the module doesn't exist, create a minimal stub that compiles (but this should NOT happen — find the actual file)

Also fix the hardcoded match/switch case extraction:
File: /home/z/my-project/vuma/src/vuma/src/pipeline.rs
Search for `case_value = 0i64` or `hardcoded` — fix it to extract the actual case value from the SCG/AST match arm.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-core
Verify: all core tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W0-T7.
```

### W0-T8: Wave 0 Verification Gate

```
Task ID: W0-T8
Agent: general-purpose

After ALL W0-T1 through W0-T7 tasks are complete, run the full verification gate:

cd /home/z/my-project/vuma && source "$HOME/.cargo/env"

1. cargo check --workspace — must produce ZERO errors and ZERO warnings
2. cargo clippy --workspace — must produce ZERO warnings
3. rg "TODO|FIXME" --type rust src/ — must return ZERO results
4. cargo test --workspace — must have ZERO failures

If any check fails, identify the exact error and report it. Do NOT fix it yourself — report back so the specific task owner can fix it.

If all checks pass, commit and push:
  git add -A
  git commit -m "Wave 0: Eliminate all warnings, TODOs, dead code, and placeholder code"
  git push origin main

Append results to /home/z/my-project/worklog.md with Task ID W0-T8.
```

---

## WAVE 1: ELF Relocation Support for All 8 ISAs

**Goal**: Add complete ELF relocation type constants and relocation emission for all 8 ISAs, enabling multi-object linking.

**Subagents**: 8 (one per ISA, all independent)

### W1-T1 through W1-T6: ISA Relocation Support

```
Task ID: W1-T{1-6}
Agent: full-stack-developer

You are adding ELF relocation support for a specific ISA to the VUMA codegen emit module.

Reference file: /home/z/my-project/vuma/src/codegen/src/emit.rs
The AArch64 relocations are already defined at lines 141-154:
  const R_AARCH64_CALL26: u32 = 283;
  const R_AARCH64_JUMP26: u32 = 282;          // #[allow(dead_code)]
  const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275; // #[allow(dead_code)]
  const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286; // #[allow(dead_code)]

Your ISA assignment: [FILL IN: x86_64 | riscv64 | mips64 | ppc64 | loongarch64 | arm32]

Step 1: Add relocation type constants for your ISA after the AArch64 block (around line 156).
Use the official ELF spec values:

  **x86_64** (EM_X86_64 = 62):
  - R_X86_64_64 = 1           // S + A (absolute 64-bit)
  - R_X86_64_PC32 = 2         // S + A - P (PC-relative 32-bit)
  - R_X86_64_PLT32 = 4        // L + A - P (PLT-relative 32-bit, for calls)
  - R_X86_64_32 = 10          // S + A (absolute 32-bit)
  - R_X86_64_32S = 11         // S + A (sign-extended 32-bit)

  **RISC-V64** (EM_RISCV = 243):
  - R_RISCV_CALL = 18         // Auipc + Jalr pair
  - R_RISCV_CALL_PLT = 19     // Auipc + Jalr pair (PLT)
  - R_RISCV_PCREL_HI20 = 23   // PC-relative high 20 bits
  - R_RISCV_PCREL_LO12_I = 24 // PC-relative low 12 bits (I-type)
  - R_RISCV_PCREL_LO12_S = 25 // PC-relative low 12 bits (S-type)
  - R_RISCV_HI20 = 26         // Absolute high 20 bits
  - R_RISCV_LO12_I = 27       // Absolute low 12 bits (I-type)
  - R_RISCV_LO12_S = 28       // Absolute low 12 bits (S-type)
  - R_RISCV_JAL = 2           // J-type relocation
  - R_RISCV_BRANCH = 16       // B-type relocation

  **MIPS64** (EM_MIPS = 8):
  - R_MIPS_26 = 4             // 26-bit jump target
  - R_MIPS_32 = 2             // 32-bit absolute
  - R_MIPS_64 = 18            // 64-bit absolute
  - R_MIPS_HI16 = 5           // High 16 bits
  - R_MIPS_LO16 = 6           // Low 16 bits
  - R_MIPS_CALL16 = 11        // Call through GOT
  - R_MIPS_GPREL16 = 7        // GP-relative 16-bit

  **PowerPC64** (EM_PPC64 = 21):
  - R_PPC64_ADDR64 = 38       // 64-bit absolute
  - R_PPC64_ADDR32 = 20       // 32-bit absolute
  - R_PPC64_REL24 = 10        // PC-relative 24-bit (branch)
  - R_PPC64_REL32 = 26        // PC-relative 32-bit
  - R_PPC64_CALL24 = 82       // Call through TOC

  **LoongArch64** (EM_LOONGARCH = 258):
  - R_LARCH_64 = 79           // 64-bit absolute
  - R_LARCH_32 = 77           // 32-bit absolute
  - R_LARCH_B26 = 69          // 26-bit branch
  - R_LARCH_PCALA_HI20 = 44   // PC-aligned high 20 bits
  - R_LARCH_PCALA_LO12 = 45   // PC-aligned low 12 bits
  - R_LARCH_CALL36 = 89       // 36-bit call

  **ARM32** (EM_ARM = 40):
  - R_ARM_CALL = 28            // PC-relative 24-bit call
  - R_ARM_JUMP24 = 29          // PC-relative 24-bit jump
  - R_ARM_MOVW_ABS_NC = 43     // MOVW absolute 16-bit
  - R_ARM_MOVT_ABS = 44        // MOVT absolute 16-bit
  - R_ARM_REL32 = 3            // PC-relative 32-bit
  - R_ARM_ABS32 = 2            // Absolute 32-bit

Step 2: Add the EM_* machine type constant for your ISA if not already defined.

Step 3: In the `emit_obj` function (search for where R_AARCH64_CALL26 rela entries are created),
add a match arm for your ISA's call relocation. The pattern should match on the ISA name
or BackendKind and emit the appropriate rela entry type.

Step 4: Add tests in the test module at the bottom of emit.rs:
  - Test that your ISA's relocation constants are defined correctly
  - Test that emit_obj produces correct rela entries for your ISA

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-codegen
Verify: all codegen tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W1-T{n}.
```

### W1-T7: Wave 1 Verification Gate

Same as W0-T8 pattern but with message "Wave 1: ELF relocations for all 8 ISAs"

---

## WAVE 2: Standard Library — Missing Modules

**Goal**: Add the 5 missing stdlib modules (fs, path, env, thread, error) and fix bare-metal I/O stubs.

**Subagents**: 7 (all independent)

### W2-T1: Add `fs.rs` — Filesystem Module

```
Task ID: W2-T1
Agent: full-stack-developer

Create a production-quality filesystem module for the VUMA standard library.

File: /home/z/my-project/vuma/src/std/src/fs.rs

Implement the following (delegating to std::fs on hosted targets with BD annotations):

1. `VumaFile` — wrapper around `std::fs::File` with BD tracking:
   - `open(path: &str) -> Result<VumaFile, VumaIoError>`
   - `create(path: &str) -> Result<VumaFile, VumaIoError>`
   - `read(&mut self, buf: &mut [u8]) -> Result<usize, VumaIoError>`
   - `write(&mut self, buf: &[u8]) -> Result<(), VumaIoError>`
   - `metadata(&self) -> Result<VumaMetadata, VumaIoError>`
   - `set_len(&self, size: u64) -> Result<(), VumaIoError>`
   - `sync_all(&self) -> Result<(), VumaIoError>`
   - `sync_data(&self) -> Result<(), VumaIoError>`

2. `VumaMetadata` — file metadata:
   - `size: u64`, `is_dir: bool`, `is_file: bool`, `is_symlink: bool`
   - `modified: Option<VumaInstant>`, `accessed: Option<VumaInstant>`, `created: Option<VumaInstant>`
   - `permissions: VumaPermissions`

3. `VumaDir` — directory iteration:
   - `read_dir(path: &str) -> Result<VumaDir, VumaIoError>`
   - `next_entry(&mut self) -> Option<Result<VumaDirEntry, VumaIoError>>`

4. Free functions:
   - `remove_file(path: &str) -> Result<(), VumaIoError>`
   - `remove_dir(path: &str) -> Result<(), VumaIoError>`
   - `remove_dir_all(path: &str) -> Result<(), VumaIoError>`
   - `rename(from: &str, to: &str) -> Result<(), VumaIoError>`
   - `copy(from: &str, to: &str) -> Result<u64, VumaIoError>`
   - `create_dir(path: &str) -> Result<(), VumaIoError>`
   - `create_dir_all(path: &str) -> Result<(), VumaIoError>`
   - `canonicalize(path: &str) -> Result<String, VumaIoError>`
   - `exists(path: &str) -> bool`
   - `hard_link(src: &str, dst: &str) -> Result<(), VumaIoError>`
   - `soft_link(src: &str, dst: &str) -> Result<(), VumaIoError>`
   - `read_link(path: &str) -> Result<String, VumaIoError>`

5. `VumaPermissions` — wrapping std::fs::Permissions with readonly() etc.

6. Minimum 20 unit tests covering: file creation, read/write round-trip, directory operations,
   metadata queries, error cases (file not found, permission denied).

7. Add `pub mod fs;` to /home/z/my-project/vuma/src/std/src/lib.rs

Follow the existing code style in the stdlib (check io.rs, net.rs for patterns).
BD annotations should track which regions own file handles (CapD tracking).

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-std
Verify: all std tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W2-T1.
```

### W2-T2: Add `path.rs` — Path Manipulation Module

```
Task ID: W2-T2
Agent: full-stack-developer

Create a production-quality path manipulation module for the VUMA standard library.

File: /home/z/my-project/vuma/src/std/src/path.rs

Implement (delegating to std::path on hosted targets):

1. `VumaPath` — borrowed path wrapper (like std::path::Path):
   - `new(s: &str) -> &VumaPath`
   - `as_str(&self) -> &str`
   - `parent(&self) -> Option<&VumaPath>`
   - `file_name(&self) -> Option<&str>`
   - `extension(&self) -> Option<&str>`
   - `file_stem(&self) -> Option<&str>`
   - `join(&self, other: &str) -> VumaPathBuf`
   - `with_extension(&self, ext: &str) -> VumaPathBuf`
   - `with_file_name(&self, name: &str) -> VumaPathBuf`
   - `is_absolute(&self) -> bool`
   - `is_relative(&self) -> bool`
   - `has_root(&self) -> bool`
   - `starts_with(&self, other: &VumaPath) -> bool`
   - `ends_with(&self, other: &VumaPath) -> bool`
   - `strip_prefix(&self, prefix: &VumaPath) -> Option<&VumaPath>`
   - `components(&self) -> Vec<PathComponent>`
   - `iter_ancestors(&self) -> Vec<&VumaPath>`

2. `VumaPathBuf` — owned path (like std::path::PathBuf):
   - `new() -> Self`
   - `from(s: String) -> Self`
   - `push(&mut self, component: &str)`
   - `pop(&mut self) -> bool`
   - `as_path(&self) -> &VumaPath`
   - `into_string(self) -> String`
   - `into_boxed_path(self) -> Box<VumaPath>`

3. `PathComponent` enum: `Prefix(PrefixKind)`, `RootDir`, `CurDir`, `ParentDir`, `Normal(String)`
4. `PrefixKind` enum: for Windows/Unix prefix differences

5. Minimum 15 unit tests covering: path joining, parent extraction, extension handling,
   absolute/relative detection, component iteration, edge cases (trailing slashes, double slashes, .., .).

6. Add `pub mod path;` to /home/z/my-project/vuma/src/std/src/lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-std

Append your work to /home/z/my-project/worklog.md with Task ID W2-T2.
```

### W2-T3: Add `env.rs` — Environment & Process Args

```
Task ID: W2-T3
Agent: full-stack-developer

Create an environment variables and process arguments module for the VUMA standard library.

File: /home/z/my-project/vuma/src/std/src/env.rs

Implement (delegating to std::env on hosted targets):

1. `args() -> Vec<String>` — command-line arguments
2. `args_os() -> Vec<Vec<u8>>` — OS-specific arguments
3. `var(key: &str) -> Result<String, VumaEnvError>` — get environment variable
4. `var_os(key: &str) -> Option<Vec<u8>>` — OS-specific env var
5. `set_var(key: &str, value: &str)` — set environment variable
6. `remove_var(key: &str)` — remove environment variable
7. `vars() -> Vec<(String, String)>` — all environment variables
8. `current_dir() -> Result<String, VumaEnvError>` — current working directory
9. `set_current_dir(path: &str) -> Result<(), VumaEnvError>` — change working directory
10. `current_exe() -> Result<String, VumaEnvError>` — path to current executable
11. `temp_dir() -> String` — temporary directory path
12. `home_dir() -> Option<String>` — home directory (uses std::env or falls back to $HOME)

13. `VumaEnvError` enum: `VarNotFound`, `NotUnicode`, `IoError(String)`

14. Minimum 10 unit tests.

15. Add `pub mod env;` to /home/z/my-project/vuma/src/std/src/lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-std

Append your work to /home/z/my-project/worklog.md with Task ID W2-T3.
```

### W2-T4: Add `thread.rs` — Threading Module

```
Task ID: W2-T4
Agent: full-stack-developer

Create a threading module for the VUMA standard library.

File: /home/z/my-project/vuma/src/std/src/thread.rs

Implement (delegating to std::thread on hosted targets, with BD annotations for Send/Sync tracking):

1. `VumaThread` — thread handle:
   - `spawn<F, T>(f: F) -> VumaJoinHandle<T> where F: FnOnce() -> T + Send + 'static, T: Send + 'static`
   - Note: Use BD CapD annotations to verify Send-ability at the VUMA level

2. `VumaJoinHandle<T>` — join handle:
   - `join(self) -> Result<T, VumaThreadError>`
   - `is_finished(&self) -> bool`
   - `thread(&self) -> &VumaThreadInfo`

3. `VumaThreadInfo` — thread metadata:
   - `id: VumaThreadId`
   - `name: Option<String>`

4. `VumaThreadId` — unique thread identifier (wraps u64)

5. `VumaThreadBuilder` — fluent thread construction:
   - `new() -> Self`
   - `name(name: &str) -> Self`
   - `stack_size(size: usize) -> Self`
   - `spawn<F, T>(self, f: F) -> Result<VumaJoinHandle<T>, VumaThreadError>`

6. `yield_now()` — cooperative yielding
7. `sleep(duration: VumaDuration)` — thread sleep
8. `park()` / `unpark(thread: &VumaThreadInfo)` — thread parking
9. `current() -> VumaThreadInfo` — current thread info

10. `VumaThreadError` enum: `CreationFailed`, `Panicked(String)`, `JoinError`

11. Minimum 15 unit tests covering: spawn+join, builder pattern, named threads,
    thread IDs, error cases.

12. Add `pub mod thread;` to /home/z/my-project/vuma/src/std/src/lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-std

Append your work to /home/z/my-project/worklog.md with Task ID W2-T4.
```

### W2-T5: Add `error.rs` — Unified Error Chain

```
Task ID: W2-T5
Agent: full-stack-developer

Create a unified error chain module for the VUMA standard library.

File: /home/z/my-project/vuma/src/std/src/error.rs

Implement:

1. `VumaError` trait (like std::error::Error):
   - `fn description(&self) -> &str`
   - `fn cause(&self) -> Option<&dyn VumaError>`
   - `fn source(&self) -> Option<&dyn VumaError>` (chain)
   - `fn kind(&self) -> VumaErrorKind`

2. `VumaErrorKind` enum:
   - `Io`, `Net`, `Parse`, `Runtime`, `Verification`, `Codegen`, `NotFound`,
   - `PermissionDenied`, `InvalidInput`, `TimedOut`, `Interrupted`,
   - `OutOfMemory`, `BufferOverflow`, `CapacityOverflow`

3. `VumaErrorChain` — chainable error with context:
   - `new(kind: VumaErrorKind, message: String) -> Self`
   - `with_source(self, source: Box<dyn VumaError>) -> Self`
   - `with_context(self, ctx: &str) -> Self`
   - `chain(&self) -> Vec<&dyn VumaError>` — iterate error chain
   - `root_cause(&self) -> &dyn VumaError` — find root error

4. `VumaResult<T>` type alias: `Result<T, VumaErrorChain>`

5. `From` impls: `From<std::io::Error>`, `From<String>`, `From<&str>`

6. Minimum 10 unit tests covering: error creation, chaining, context addition,
   root cause finding, kind classification.

7. Add `pub mod error;` to /home/z/my-project/vuma/src/std/src/lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-std

Append your work to /home/z/my-project/worklog.md with Task ID W2-T5.
```

### W2-T6: Fix Bare-Metal I/O — Enable Real MMIO

```
Task ID: W2-T6
Agent: full-stack-developer

You are replacing the bare-metal UART stubs in the VUMA standard library with real volatile MMIO access.

File: /home/z/my-project/vuma/src/std/src/io.rs

Current stubs (lines ~655-683):
- `read_uart_byte()` returns `Ok(0)` — needs real volatile read from data register
- `uart_rx_ready()` returns `true` — needs real FR register bit 4 check

Fix `read_uart_byte`:
```rust
fn read_uart_byte(&mut self) -> VumaIoResult<u8> {
    if !self.uart_rx_ready() {
        return Err(VumaIoError::WouldBlock);
    }
    let dr = (self.mmio_base + 0x00) as *const u32;
    let byte = unsafe { core::ptr::read_volatile(dr) as u8 };
    Ok(byte)
}
```

Fix `write_uart_byte`:
```rust
fn write_uart_byte(&mut self, byte: u8) -> VumaIoResult<()> {
    // Wait until transmit FIFO has space (FR bit 5 = TXFF)
    let fr = (self.mmio_base + 0x18) as *const u32;
    while (unsafe { core::ptr::read_volatile(fr) } & (1 << 5)) != 0 {
        core::hint::spin_loop();
    }
    let dr = (self.mmio_base + 0x00) as *mut u32;
    unsafe { core::ptr::write_volatile(dr, byte as u32) };
    Ok(())
}
```

Fix `uart_rx_ready`:
```rust
fn uart_rx_ready(&self) -> bool {
    let fr = (self.mmio_base + 0x18) as *const u32;
    let fr_val = unsafe { core::ptr::read_volatile(fr) };
    // FR bit 4 = RXFE (receive FIFO empty); ready when NOT empty
    (fr_val & (1 << 4)) == 0
}
```

IMPORTANT: These changes should only activate on bare-metal targets. The existing Linux
path (using std::io) must remain unchanged. Use cfg attributes:
- `#[cfg(not(target_os = "none"))]` for the Linux path
- `#[cfg(target_os = "none")]` for the bare-metal volatile path

Also fix the legacy Stdin::read stub (line ~1602):
```rust
pub fn read(&mut self, buf_len: usize) -> Result<Vec<u8>, String> {
    // This was returning a zeroed buffer; delegate to real stdin
    let mut buf = vec![0u8; buf_len];
    match std::io::stdin().read(&mut buf) {
        Ok(n) => Ok(buf[..n].to_vec()),
        Err(e) => Err(e.to_string()),
    }
}
```

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-std
Verify: all std tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W2-T6.
```

### W2-T7: Wave 2 Verification Gate

Same pattern — full verification + commit "Wave 2: Complete stdlib (fs, path, env, thread, error, real MMIO)"

---

## WAVE 3: Pi5 BCM2712 Platform — Real Hardware Target

**Goal**: Make the Pi5 bare-metal crate target the actual BCM2712 SoC (not BCM2711) with real exception handlers, interrupt controller, and QEMU raspi4b support.

**Subagents**: 6

### W3-T1: Update QEMU Target to raspi4b + Add x86_64/RISC-V QEMU Targets

```
Task ID: W3-T1
Agent: full-stack-developer

You are updating the VUMA build system QEMU targets.

Files:
- /home/z/my-project/vuma/Makefile
- /home/z/my-project/vuma/justfile

Changes:

1. Change Pi5 QEMU machine from `raspi3b` to `raspi4b`:
   Makefile line 162: `qemu-system-aarch64 -M raspi4b -serial stdio -kernel $(PI5_IMG) -s -S`
   Makefile line 170: `qemu-system-aarch64 -M raspi4b -serial stdio -kernel $(PI5_IMG)`
   justfile line 137: `qemu-system-aarch64 -M raspi4b -serial stdio -kernel src/pi5/kernel8.img -s -S`
   justfile line 141: `qemu-system-aarch64 -M raspi4b -serial stdio -kernel src/pi5/kernel8.img`

2. Add x86_64 QEMU run target:
   Makefile: add `x86-64-run: build` target:
     `qemu-system-x86_64 -kernel target/debug/vuma-x86_64 -serial stdio`
   justfile: add `x86-64-run` recipe similarly

3. Add RISC-V64 QEMU run target:
   Makefile: add `riscv64-run: build` target:
     `qemu-system-riscv64 -machine virt -kernel target/debug/vuma-riscv64 -serial stdio -nographic`
   justfile: add `riscv64-run` recipe similarly

4. Add `--gc-sections` to Pi5 linker flags in the linker script or objcopy step
   (search for `--no-gc-sections` and remove the `no-` prefix)

Run: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && make check && just check

Append your work to /home/z/my-project/worklog.md with Task ID W3-T1.
```

### W3-T2: BCM2712 GIC-400 Interrupt Controller Driver

```
Task ID: W3-T2
Agent: full-stack-developer

Create a GIC-400 interrupt controller driver for the BCM2712 SoC.

File: /home/z/my-project/vuma/src/pi5/src/gic.rs (new file)

The BCM2712 uses a GIC-400 (Generic Interrupt Controller) with:
- Distributor base: PERIPHERAL_BASE + 0x0000_B400 (adjust for BCM2712 high-peripheral mode)
- CPU Interface base: PERIPHERAL_BASE + 0x0000_B500

Implement:

1. `Gic400` struct with distributor and cpu_interface base addresses
2. `init(&mut self)` — initialize GIC:
   - Enable distributor (GICD_CTLR = 1)
   - Set all SPI interrupts to group 0 (GICD_IGROUPR)
   - Enable CPU interface (GICC_CTLR = 1)
   - Set priority mask to lowest (GICC_PMR = 0xFF)
3. `enable_irq(irq: u32)` — enable specific interrupt:
   - Set enable bit in GICD_ISENABLER
4. `disable_irq(irq: u32)` — disable specific interrupt
5. `acknowledge_irq(&self) -> u32` — read IAR (Interrupt Acknowledge Register)
6. `end_of_irq(irq: u32)` — write to EOIR (End of Interrupt Register)
7. `set_priority(irq: u32, priority: u8)` — set interrupt priority in GICD_IPRIORITYR
8. `get_pending_irq(&self) -> Option<u32>` — check for pending interrupts

BCM2712-specific interrupt assignments:
- Timer (ARM Generic Timer): IRQ 30
- UART (PL011): IRQ 57
- GPIO: IRQ 145-152
- PCIe: IRQ 224+

Add `pub mod gic;` to /home/z/my-project/vuma/src/pi5/src/lib.rs

Minimum 10 unit tests.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-pi5

Append your work to /home/z/my-project/worklog.md with Task ID W3-T2.
```

### W3-T3: Real Exception Handlers (Replace Spin-Loops)

```
Task ID: W3-T3
Agent: full-stack-developer

You are replacing the spin-loop exception handlers in the VUMA Pi5 bare-metal crate with real handlers that save/restore context and route interrupts.

File: /home/z/my-project/vuma/src/pi5/src/boot.rs

Current state: All 16 AArch64 exception vector entries are stub `.align 7` blocks that just spin (WFE loop).

Replace with a proper exception handling framework:

1. Create /home/z/my-project/vuma/src/pi5/src/exception.rs (new file):

2. `ExceptionContext` struct (saved on exception entry):
   ```rust
   #[repr(C)]
   pub struct ExceptionContext {
       // General-purpose registers
       pub x0: u64, pub x1: u64, /* ... */ pub x30: u64,
       // Special registers
       pub spsr: u64,   // Saved Program Status Register
       pub elr: u64,    // Exception Link Register (return address)
       pub esr: u64,    // Exception Syndrome Register
       pub far: u64,    // Fault Address Register
   }
   ```

3. `ExceptionType` enum: `Synchronous`, `Irq`, `Fiq`, `SError`

4. Handler functions for each exception type:
   - `handle_sync(ctx: &mut ExceptionContext)` — decode ESR, handle traps, page faults
   - `handle_irq(ctx: &mut ExceptionContext)` — acknowledge GIC, dispatch to handler, EOI
   - `handle_fiq(ctx: &mut ExceptionContext)` — fast interrupt (typically secure monitor)
   - `handle_serror(ctx: &mut ExceptionContext)` — system error (RAS)

5. `install_handlers()` — writes exception vector base to VBAR_EL1

6. In boot.rs, replace the stub vectors with:
   ```asm
   .align 7
   _vec_el1_sync_current:
       stp x0, x1, [sp, #-16]!
       mrs x0, esr_el1
       mrs x1, elr_el1
       /* save context, call handle_sync, restore context */
       eret
   ```
   (Use naked_asm for each vector entry, calling into the Rust handler)

7. Add `pub mod exception;` to lib.rs

Minimum 8 unit tests.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-pi5

Append your work to /home/z/my-project/worklog.md with Task ID W3-T3.
```

### W3-T4: Update UART Base for BCM2712 (Pi5)

```
Task ID: W3-T4
Agent: full-stack-developer

You are updating the UART base address and related constants for the actual BCM2712 SoC (Raspberry Pi 5).

Files to update:
1. /home/z/my-project/vuma/src/std/src/io.rs — UART_PL011_BASE constant (line 613)
   Change from 0xFE201000 (BCM2711/Pi4) to 0xFE201000 for BCM2712
   Wait — check this carefully. The BCM2712 Pi5 uses a different UART:
   - BCM2712 has PL011 at a DIFFERENT address than BCM2711
   - Pi5 BCM2712: PL011 UART is at 0x10_0000 offset from peripheral base
   - In high-peripheral mode: 0x1C_0010_0000 + 0x10_0000 = not quite right
   Research: The BCM2712 uses a custom UART. Check the Raspberry Pi 5 documentation.

   The key change: In BCM2712 high-peripheral mode:
   - PERIPHERAL_BASE_HIGH = 0x7C00_0000 (from platform.rs)
   - UART offset = 0x0010_A000 (for PL011)
   - So UART = 0x7C00_0000 + 0x0010_A000 = 0x7C10_A000

   But wait — the platform.rs already has:
   - `UART_BASE_OFFSET: u64 = 0x010A_0000` (line 100)
   
   So: io.rs should use `PERIPHERAL_BASE_HIGH + UART_BASE_OFFSET` instead of a hardcoded constant.

   Fix: Remove the hardcoded UART_PL011_BASE and compute it dynamically from the platform constants.

2. /home/z/my-project/vuma/src/pi5/src/uart.rs — Check if it uses hardcoded addresses
   Update any BCM2711-specific addresses to use platform.rs constants.

3. /home/z/my-project/vuma/src/pi5/src/gpio.rs — Check for BCM2711-specific offsets
   The Pi5 uses RP1 GPIO controller at a different address than Pi4.
   platform.rs already has RP1_GPIO_BASE = 0x1F_0001_0000
   Update gpio.rs to use this instead of any BCM2711 GPIO offsets.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-pi5 && cargo test -p vuma-std

Append your work to /home/z/my-project/worklog.md with Task ID W3-T4.
```

### W3-T5: Pi5 Linker Script Update for BCM2712 Memory Map

```
Task ID: W3-T5
Agent: full-stack-developer

You are updating the Pi5 linker script for the BCM2712 memory map.

File: /home/z/my-project/vuma/src/pi5/link.ld

Current state: 8 MiB RAM at 0x80000 (Pi3/Pi4-era)

BCM2712 (Pi5) memory map:
- RAM starts at 0x0000_0000 (lower 3 GB)
- Or 0x0000_0000_0000_0000 (with ARM Local at 0x7C00_0000_0000 in high mode)
- The GPU firmware loads the kernel at 0x80000 even on Pi5 (this is the default)
- However, Pi5 supports up to 8 GB RAM
- MMIO peripherals in high-peripheral mode start at 0x7C00_0000

Updates needed:
1. Keep RAM at 0x80000 (this is where the GPU loads the kernel)
2. Increase available RAM to match Pi5's 4 GB or 8 GB
3. Add MMIO section at the BCM2712 high-peripheral address
4. Add GIC distributor and CPU interface sections
5. Increase per-core stack to 128 KiB (Pi5 has more RAM)
6. Add .got and .got.plt sections for position-independent code support
7. Add .note.gnu.build-id section
8. Remove --no-gc-sections from the objcopy step (check Makefile/justfile)

Also update /home/z/my-project/vuma/src/pi5/src/boot.rs if the BSS start/end
symbols need to change due to linker script modifications.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-pi5

Append your work to /home/z/my-project/worklog.md with Task ID W3-T5.
```

### W3-T6: Wave 3 Verification Gate

Same pattern — commit "Wave 3: Pi5 BCM2712 platform, GIC driver, real exception handlers, real MMIO"

---

## WAVE 4: IVE Hardening — Extract All SCG Events, Fix Default BD Pass-Through

**Goal**: Make the IVE extraction cover ALL SCG node types (not just Allocation/Deallocation), and ensure no invariant check passes trivially.

**Subagents**: 5

### W4-T1: Extract Lock/Channel Events for Liveness

```
Task ID: W4-T1
Agent: full-stack-developer

You are expanding the SCG→Liveness event extraction in the VUMA IVE to cover lock and channel operations.

File: /home/z/my-project/vuma/src/ive/src/verification.rs

Current code (lines ~206-243):
    match node.node_type {
        NodeType::Allocation => { ... }     // adds ResourceEvent::Allocate
        NodeType::Deallocation => { ... }   // adds ResourceEvent::Deallocate
        NodeType::Access => { ... }         // no-op
        _ => {}                             // ALL other types IGNORED
    }

Expand to also extract:
1. `NodeType::Computation` — check if it's a lock acquire/release:
   - Search the node's label or annotations for "lock_acquire" / "lock_release" / "mutex_lock" / "mutex_unlock"
   - Add `ResourceEvent::LockAcquire { region, lock_id }` and `ResourceEvent::LockRelease { region, lock_id }`
2. `NodeType::Computation` — check for channel send/recv:
   - Add `ResourceEvent::ChannelSend { channel_id }` and `ResourceEvent::ChannelRecv { channel_id }`
3. `NodeType::Effect` — treat as potential side-effect that affects resource state
4. `NodeType::Cast` — check for capability transitions (CapD weakening/strengthening)

You will need to:
1. Add new `ResourceEvent` variants in /home/z/my-project/vuma/src/ive/src/liveness.rs:
   - `LockAcquire { region: String, lock_id: String }`
   - `LockRelease { region: String, lock_id: String }`
   - `ChannelSend { channel_id: String }`
   - `ChannelRecv { channel_id: String }`
2. Update the liveness verifier to handle these new events:
   - Lock acquire/release affects deadlock detection (add to Tarjan SCC analysis)
   - Channel send/recv affects message completeness check
3. Update `scg_extract_liveness` in verification.rs to populate the new events
4. Add at least 5 tests for lock-aware and channel-aware liveness verification

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-ive

Append your work to /home/z/my-project/worklog.md with Task ID W4-T1.
```

### W4-T2: Fix Exclusivity base_address=0 Problem

```
Task ID: W4-T2
Agent: full-stack-developer

You are fixing the IVE exclusivity verifier to use real address ranges instead of base_address=0 for all accesses.

File: /home/z/my-project/vuma/src/ive/src/verification.rs

Current problem: When extracting SCG access nodes for exclusivity checking,
all accesses get `base_address = 0` because the SCG doesn't track concrete addresses.
This means overlap detection always returns true — every pair of same-region accesses
is checked for conflicts, which is correct but imprecise.

Fix by implementing symbolic address ranges:

1. Add a `SymbolicAddress` type in /home/z/my-project/vuma/src/ive/src/exclusivity.rs:
   ```rust
   pub enum SymbolicAddress {
       /// Concrete address (from static allocation or known offset)
       Concrete { base: u64, size: u64 },
       /// Symbolic offset from a region base (e.g., field offset)
       OffsetFromRegion { region: String, offset: u64, size: u64 },
       /// Unknown — must conservatively overlap with everything
       Unknown { region: String },
   }
   ```

2. Implement `overlaps(&self, other: &SymbolicAddress) -> bool`:
   - Concrete vs Concrete: actual range overlap
   - OffsetFromRegion same region: offset overlap check
   - OffsetFromRegion different region: no overlap
   - Unknown: always overlaps (conservative)

3. Update the extraction code in verification.rs to build SymbolicAddress from SCG data:
   - Allocation nodes → OffsetFromRegion with region name and offset 0
   - Access nodes → OffsetFromRegion with offset from the node's BD/annotations
   - Unknown nodes → Unknown variant

4. Update the exclusivity verifier to use `SymbolicAddress::overlaps()` instead of
   always checking every pair.

5. Add at least 5 tests for address overlap detection with symbolic ranges.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-ive

Append your work to /home/z/my-project/worklog.md with Task ID W4-T2.
```

### W4-T3: Fix Interpretation Default BD Pass-Through

```
Task ID: W4-T3
Agent: full-stack-developer

You are fixing the IVE interpretation verifier so it doesn't trivially pass when no BD map is provided.

File: /home/z/my-project/vuma/src/ive/src/interpretation.rs

Current problem: When no BD map is available, the interpreter uses default BDs:
  - RepD: Byte{8,8}
  - CapD: all capabilities
  - RelD: empty
This means interpretation checks trivially pass because every CapD transition is
allowed (all → all is valid) and every RepD is compatible (Byte is compatible with everything).

Fix:

1. When no BD map is provided, emit a `VerificationWarning::MissingBDMap` instead of silently using defaults.
2. For each write-read pair without BD information:
   - Mark it as "unverified" rather than "pass"
   - Add it to a new `UnverifiedPairs` list in the verification result
3. Add a strictness level to interpretation verification:
   ```rust
   pub enum InterpretationStrictness {
       /// Default BDs are acceptable (current behavior, for backward compat)
       Permissive,
       /// Unverified pairs produce warnings
       Moderate,
       /// Unverified pairs produce errors (fail the verification)
       Strict,
   }
   ```
4. Default to `Moderate` — produce warnings but don't fail.

5. Add at least 5 tests for strictness levels and unverified pair tracking.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-ive

Append your work to /home/z/my-project/worklog.md with Task ID W4-T3.
```

### W4-T4: Expand Origin/Cleanup SCG Extraction

```
Task ID: W4-T4
Agent: full-stack-developer

You are expanding the SCG extraction for the Origin and Cleanup invariants to cover more node types.

File: /home/z/my-project/vuma/src/ive/src/verification.rs

Current gaps:
- Origin extraction (lines ~422-488): Only creates OriginRegion for NodeType::Allocation
  Should also track: Cast (pointer derivation), Computation (arithmetic on pointers),
  Effect (side effects that create new pointers)

- Cleanup extraction (lines ~488-530): Only maps Allocation→Acquire, Deallocation→Release
  Should also track: Lock operations (acquire/release), File handles, Network connections,
  Channel endpoints

Fix both:

1. Origin extraction — add handling for:
   - `NodeType::Cast` → record pointer derivation in the provenance forest
   - `NodeType::Computation` → record pointer arithmetic as derivation step
   - `NodeType::Access` → record access as a use point (affects liveness origin)

2. Cleanup extraction — add handling for:
   - Any node with "lock" in label → Acquire/Release pair
   - Any node with "open"/"close" in label → Acquire/Release pair
   - Any node with "connect"/"disconnect" → Acquire/Release pair

3. Add at least 5 tests for expanded extraction.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-ive

Append your work to /home/z/my-project/worklog.md with Task ID W4-T4.
```

### W4-T5: Wave 4 Verification Gate

Same pattern — commit "Wave 4: IVE hardening — full SCG event extraction, symbolic addresses, strict interpretation"

---

## WAVE 5: Proof System — Typed IDs, Serialization, Composition

**Goal**: Upgrade the proof system from String-typed judgments to typed IDs, add proof serialization, and enable cross-invariant proof composition.

**Subagents**: 5

### W5-T1: Replace String Fields with Typed IDs in Judgments

```
Task ID: W5-T1
Agent: full-stack-developer

You are replacing all String-typed fields in the VUMA proof system's Judgment enum with typed ID newtypes.

Files to modify:
1. /home/z/my-project/vuma/src/proof/src/judgment.rs — the main Judgment enum
2. Any file in /home/z/my-project/vuma/src/proof/ that constructs or matches on Judgment variants

Step 1: Define typed ID newtypes (add to judgment.rs or a new ids.rs):
   ```rust
   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct RegionId(pub u64);

   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct ResourceId(pub u64);

   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct PointerId(pub u64);

   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct VariableId(pub u64);

   #[derive(Debug, Clone, PartialEq, Eq, Hash)]
   pub struct EventId(pub u64);
   ```

Step 2: Replace all String fields in Judgment:
   - `Allocated { region: String }` → `Allocated { region: RegionId }`
   - `Live { region: String }` → `Live { region: RegionId }`
   - `Freed { region: String }` → `Freed { region: RegionId }`
   - `Exclusive { resource: String }` → `Exclusive { resource: ResourceId }`
   - `Shared { resource: String, count: usize }` → `Shared { resource: ResourceId, count: usize }`
   - `Derived { pointer: String, from: String, region: String }` → `Derived { pointer: PointerId, from: PointerId, region: RegionId }`
   - `InBounds { pointer: String, offset: i64, size: i64 }` → `InBounds { pointer: PointerId, offset: i64, size: i64 }`
   - `Initialized { variable: String }` → `Initialized { variable: VariableId }`
   - `PreservesCapD { resource: String, ... }` → `PreservesCapD { resource: ResourceId, ... }`
   - `TemporalOrder { event_a: String, event_b: String }` → `TemporalOrder { event_a: EventId, event_b: EventId }`

Step 3: Update ALL proof modules that construct or pattern-match on Judgment:
   - liveness_proofs.rs, exclusivity_proofs.rs, interpretation_proofs.rs,
     origin_proofs.rs, cleanup_proofs.rs, checker.rs, rules.rs, tactics.rs, counterexample.rs

Step 4: Add Display impls for the ID types (showing both the ID number and a debug name if available).

Step 5: Run ALL proof tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-proof

Verify: all 182+ tests pass.

Append your work to /home/z/my-project/worklog.md with Task ID W5-T1.
```

### W5-T2: Proof Serialization & Persistence

```
Task ID: W5-T2
Agent: full-stack-developer

You are adding proof serialization and persistence to the VUMA proof system.

Files to create/modify:
1. /home/z/my-project/vuma/src/proof/src/serialize.rs (new file)
2. /home/z/my-project/vuma/src/proof/src/lib.rs (add mod serialize)

Implement:

1. `ProofFormat` enum: `Json`, `Binary`, `Markdown`

2. `serialize_proof(proof: &Proof, format: ProofFormat) -> Vec<u8>`:
   - JSON: serde_json serialization with human-readable proof structure
   - Binary: bincode or custom compact binary format
   - Markdown: human-readable proof document with inference rules shown

3. `deserialize_proof(data: &[u8], format: ProofFormat) -> Result<Proof, ProofError>`

4. `ProofStore` — file-based proof storage:
   - `new(path: &str) -> Result<Self, ProofError>`
   - `store(&mut self, proof: &Proof, id: &str) -> Result<(), ProofError>`
   - `load(&mut self, id: &str) -> Result<Proof, ProofError>`
   - `list(&self) -> Vec<String>` — list stored proof IDs
   - `delete(&mut self, id: &str) -> Result<(), ProofError>`

5. `ProofExporter` — export proofs for external verification:
   - `export_as_coq(proof: &Proof) -> String` — generate Coq-verified proof script
   - `export_as_lean(proof: &Proof) -> String` — generate Lean proof script
   - `export_as_isabelle(proof: &Proof) -> String` — generate Isabelle proof script

6. Add `#[derive(Serialize, Deserialize)]` to Proof, ProofStep, Judgment, and related types.
   You'll need to add `serde` to the proof crate's Cargo.toml.

7. Minimum 10 tests: round-trip serialization, proof store CRUD, export format validity.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-proof

Append your work to /home/z/my-project/worklog.md with Task ID W5-T2.
```

### W5-T3: Cross-Invariant Proof Composition

```
Task ID: W5-T3
Agent: full-stack-developer

You are adding cross-invariant proof composition to the VUMA proof system.

Currently, proofs for each of the 5 invariants are independent. However, invariants have dependencies:
- Origin must hold before Liveness can be verified (need valid provenance for leak detection)
- Exclusivity depends on Liveness (dead resources can't be exclusive)
- Cleanup depends on Liveness (must know what's live before checking what's freed)
- Interpretation depends on Origin (must know pointer provenance to check RepD compatibility)

Files to create/modify:
1. /home/z/my-project/vuma/src/proof/src/composition.rs (new file)

Implement:

1. `ProofDependency` struct:
   ```rust
   pub struct ProofDependency {
       pub source_invariant: InvariantKind,
       pub target_invariant: InvariantKind,
       pub dependency_type: DependencyType,
   }
   pub enum DependencyType {
       /// Target proof requires source proof's conclusions as premises
       RequiresPremise,
       /// Target proof assumes source invariant holds
       AssumesHolds,
       /// Target proof strengthens source proof's conclusions
       Strengthens,
   }
   ```

2. `ProofComposition` struct that manages the dependency graph:
   - `dependencies() -> Vec<ProofDependency>` — returns the fixed dependency graph
   - `verify_composition(proofs: HashMap<InvariantKind, Proof>) -> CompositionResult`
   - `check_premise_satisfaction(source: &Proof, target: &Proof) -> bool`

3. `CompositeProof` struct:
   - `individual_proofs: HashMap<InvariantKind, Proof>`
   - `dependency_satisfaction: HashMap<(InvariantKind, InvariantKind), bool>`
   - `overall_verdict: CompositeVerdict`

4. The verification order must match IVE's dependency order:
   Origin → Liveness → Exclusivity → Interpretation → Cleanup

5. Minimum 8 tests: dependency graph structure, premise satisfaction, composition verification.

6. Add `pub mod composition;` to lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-proof

Append your work to /home/z/my-project/worklog.md with Task ID W5-T3.
```

### W5-T4: Additional Inference Rules & Tactics

```
Task ID: W5-T4
Agent: full-stack-developer

You are expanding the VUMA proof system's inference rules and tactics.

Files to modify:
1. /home/z/my-project/vuma/src/proof/src/rules.rs
2. /home/z/my-project/vuma/src/proof/src/tactics.rs

Add the following inference rules:

1. `RegionSplit` — a region can be split into sub-regions, each with its own BD
2. `CapabilityWeakening` — CapD can be weakened (e.g., ReadWrite → Read)
3. `CapabilityStrengthening` — CapD can be strengthened under exclusive access
4. `ProvenanceTransitivity` — if A derives from B and B derives from C, A derives from C
5. `CleanupComposition` — cleanup on composed regions implies cleanup on parts
6. `DeadResourceExclusivity` — dead resources are vacuously exclusive
7. `TemporalTransitivity` — if A before B and B before C, then A before C

Add the following tactics:

1. `InductionOnRegion` — structural induction on region nesting
2. `CaseAnalysis` — split proof into cases based on resource state
3. `Contrapositive` — prove the contrapositive instead of the original

Minimum 8 new tests for rules and tactics.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-proof

Append your work to /home/z/my-project/worklog.md with Task ID W5-T4.
```

### W5-T5: Wave 5 Verification Gate

Same pattern — commit "Wave 5: Typed proof IDs, serialization, cross-invariant composition, new rules"

---

## WAVE 6: SCG Enhancement — Region Inference, VTable Nodes, Loop Detection

**Goal**: Make the SCG the most sophisticated intermediate representation possible.

**Subagents**: 4

### W6-T1: Region Inference & Alias Analysis

```
Task ID: W6-T1
Agent: full-stack-developer

You are adding region inference and region-based alias analysis to the VUMA SCG module.

File: /home/z/my-project/vuma/src/scg/src/region.rs (currently 218 lines — expand significantly)

Implement:

1. `RegionInference` engine:
   - `infer_regions(scg: &SCG) -> Vec<InferredRegion>`
   - Based on allocation/deallocation points and pointer flow
   - Uses the BD CapD to determine when regions can be merged (same lifetime, compatible CapD)

2. `InferredRegion`:
   ```rust
   pub struct InferredRegion {
       pub id: RegionId,
       pub nodes: Vec<NodeId>,
       pub entry_node: NodeId,
       pub exit_nodes: Vec<NodeId>,
       pub lifetime: RegionLifetime,
       pub parent: Option<RegionId>,
       pub children: Vec<RegionId>,
   }
   pub enum RegionLifetime {
       /// Lives for entire program duration
       Static,
       /// Lives from allocation to deallocation
       Scoped { alloc: NodeId, dealloc: NodeId },
       /// Lives as long as any reference exists
       ReferenceCounted { ref_nodes: Vec<NodeId> },
       /// Unknown lifetime
       Unknown,
   }
   ```

3. `RegionAliasAnalysis`:
   - `may_alias(scg: &SCG, region_a: RegionId, region_b: RegionId) -> bool`
   - Two regions may alias if they share any allocation node or if their lifetime ranges overlap
   - Uses region hierarchy: parent/child regions may alias, sibling regions don't

4. `RegionCompatibility`:
   - `can_merge(scg: &SCG, region_a: RegionId, region_b: RegionId) -> bool`
   - Regions can be merged if they have compatible lifetimes and CapD constraints allow it

5. Minimum 10 tests: region inference on simple programs, alias analysis, merge decisions.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-scg

Append your work to /home/z/my-project/worklog.md with Task ID W6-T1.
```

### W6-T2: VTable First-Class Node Type + Dispatch Refinement

```
Task ID: W6-T2
Agent: full-stack-developer

You are adding a VTable first-class node type to the VUMA SCG and refining Dispatch edges.

Files to modify:
1. /home/z/my-project/vuma/src/scg/src/node.rs — add NodeType::VTable and VTablePayload
2. /home/z/my-project/vuma/src/scg/src/edge.rs — refine EdgeKind::Dispatch with metadata
3. /home/z/my-project/vuma/src/scg/src/graph.rs — support VTable node operations
4. /home/z/my-project/vuma/src/parser/src/to_scg.rs — generate VTable nodes from AST

Implement:

1. New `NodeType::VTable` variant with `VTablePayload`:
   ```rust
   pub struct VTablePayload {
       pub trait_name: String,
       pub impl_type: String,
       pub entries: Vec<VTableEntry>,
   }
   pub struct VTableEntry {
       pub method_name: String,
       pub target_node: NodeId,
       pub signature_bd: BdId,
   }
   ```

2. Refined `EdgeKind::Dispatch`:
   ```rust
   Dispatch {
       vtable_node: NodeId,      // the VTable being dispatched through
       method_index: usize,       // which entry in the VTable
       receiver_node: NodeId,     // the object being dispatched on
   }
   ```

3. SCG methods:
   - `add_vtable(&mut self, payload: VTablePayload) -> NodeId`
   - `dispatch_targets(&self, vtable: NodeId) -> Vec<NodeId>` — all possible dispatch targets

4. In to_scg.rs, when converting impl blocks and trait method calls, create VTable nodes.

5. Minimum 8 tests.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-scg && cargo test -p vuma-parser

Append your work to /home/z/my-project/worklog.md with Task ID W6-T2.
```

### W6-T3: Loop Detection in SCG

```
Task ID: W6-T3
Agent: full-stack-developer

You are adding loop detection to the VUMA SCG module.

File: /home/z/my-project/vuma/src/scg/src/loop_detection.rs (new file)

Implement:

1. `LoopDetector` struct with methods:
   - `detect_natural_loops(scg: &SCG) -> Vec<NaturalLoop>`
   - `detect_loop_nesting(scg: &SCG) -> LoopNestingTree`
   - `detect_infinite_loops(scg: &SCG) -> Vec<NodeId>` — loops with no exit

2. `NaturalLoop`:
   ```rust
   pub struct NaturalLoop {
       pub header: NodeId,          // loop entry
       pub backedge_source: NodeId, // source of the back-edge
       pub body: Vec<NodeId>,       // all nodes in the loop body
       pub exits: Vec<NodeId>,      // nodes that exit the loop
       pub depth: usize,            // nesting depth
   }
   ```

3. `LoopNestingTree`:
   ```rust
   pub struct LoopNestingTree {
       pub loops: Vec<NaturalLoop>,
       pub parent: HashMap<usize, Option<usize>>,  // loop index → parent loop index
   }
   ```

4. Algorithm: Use Tarjan's algorithm to find SCCs in the control-flow subgraph,
   then identify back-edges and compute natural loops from the dominator tree
   (which already exists in dominance.rs).

5. Integration with LICM: Add a method that identifies loop-invariant nodes
   (nodes whose inputs are all defined outside the loop) for the optimization pass.

6. Add `pub mod loop_detection;` to /home/z/my-project/vuma/src/scg/src/lib.rs

7. Minimum 10 tests: simple loops, nested loops, infinite loops, loop-invariant detection.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-scg

Append your work to /home/z/my-project/worklog.md with Task ID W6-T3.
```

### W6-T4: Wave 6 Verification Gate

Same pattern — commit "Wave 6: SCG region inference, VTable nodes, loop detection"

---

## WAVE 7: COR Hardening — Real x86_64 Execution, Region Ownership, Integration Tests

**Goal**: Make the COR a real continuous optimization runtime, not just a framework.

**Subagents**: 4

### W7-T1: x86_64 JIT Execution in COR

```
Task ID: W7-T1
Agent: full-stack-developer

You are adding real JIT execution on x86_64 hosts to the VUMA COR runtime.

File: /home/z/my-project/vuma/src/cor/src/runtime.rs

Current state (lines 759-774): execute_code() only does real mmap+mprotect on AArch64.
On x86_64 it returns Ok(0) — a stub.

Add an x86_64 execution path:
```rust
#[cfg(all(unix, target_arch = "x86_64"))]
fn execute_code_x86_64(code: &[u8]) -> Result<i64, RuntimeError> {
    use std::ptr;
    let len = code.len();
    let page_size = 4096usize;
    let aligned_len = ((len + page_size - 1) / page_size) * page_size;

    unsafe {
        let mem = libc::mmap(
            ptr::null_mut(), aligned_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0,
        );
        if mem == libc::MAP_FAILED {
            return Err(RuntimeError::ExecutionFailed(0, "mmap failed".to_string()));
        }
        ptr::copy_nonoverlapping(code.as_ptr(), mem as *mut u8, len);

        // Make executable
        let mprotect_result = libc::mprotect(mem, aligned_len, libc::PROT_READ | libc::PROT_EXEC);
        if mprotect_result != 0 {
            libc::munmap(mem, aligned_len);
            return Err(RuntimeError::ExecutionFailed(0, "mprotect failed".to_string()));
        }

        // Call the code as a function returning i64 with no arguments
        // x86_64 SystemV ABI: result in RAX
        let func: extern "C" fn() -> i64 = std::mem::transmute(mem);
        let result = func();

        libc::munmap(mem, aligned_len);
        Ok(result)
    }
}
```

Update execute_code() to dispatch to the x86_64 path:
```rust
fn execute_code(code: &[u8]) -> Result<i64, RuntimeError> {
    if code.is_empty() { return Ok(0); }

    #[cfg(all(unix, target_arch = "aarch64"))]
    { execute_code_aarch64(code) }

    #[cfg(all(unix, target_arch = "x86_64"))]
    { execute_code_x86_64(code) }

    #[cfg(not(any(all(unix, target_arch = "aarch64"), all(unix, target_arch = "x86_64"))))]
    { let _ = code; Ok(0) }
}
```

Also fix the return value being discarded at line 310:
- Change `let _ = result;` to store the result in the profile data.

Minimum 5 new tests.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-cor

Append your work to /home/z/my-project/worklog.md with Task ID W7-T1.
```

### W7-T2: Region-Based Ownership Tracking in COR

```
Task ID: W7-T2
Agent: full-stack-developer

You are adding region-based ownership tracking to the VUMA COR, making it actually
track concurrent ownership as its name (Concurrent Ownership & Regions) suggests.

File: /home/z/my-project/vuma/src/cor/src/ownership.rs (new file)

Implement:

1. `OwnershipTracker` struct:
   ```rust
   pub struct OwnershipTracker {
       regions: HashMap<RegionId, RegionState>,
       access_log: Vec<AccessRecord>,
   }
   ```

2. `RegionState`:
   ```rust
   pub struct RegionState {
       pub id: RegionId,
       pub owner: Option<ThreadId>,
       pub access_mode: AccessMode,
       pub waiting_threads: VecDeque<ThreadId>,
   }
   pub enum AccessMode {
       Free,           // No one owns it
       SharedRead { holders: Vec<ThreadId> },
       ExclusiveWrite { holder: ThreadId },
   }
   ```

3. `AccessRecord` for tracking access history:
   ```rust
   pub struct AccessRecord {
       pub thread: ThreadId,
       pub region: RegionId,
       pub mode: AccessMode,
       pub timestamp: u64,
   }
   ```

4. Methods:
   - `acquire_read(&mut self, region: RegionId, thread: ThreadId) -> Result<(), OwnershipError>`
   - `acquire_write(&mut self, region: RegionId, thread: ThreadId) -> Result<(), OwnershipError>`
   - `release(&mut self, region: RegionId, thread: ThreadId) -> Result<(), OwnershipError>`
   - `try_acquire_read(&mut self, region: RegionId, thread: ThreadId) -> bool`
   - `try_acquire_write(&mut self, region: RegionId, thread: ThreadId) -> bool`
   - `detect_data_races(&self) -> Vec<DataRace>` — analyze access_log for conflicting accesses

5. `DataRace`:
   ```rust
   pub struct DataRace {
       pub region: RegionId,
       pub thread_a: ThreadId,
       pub thread_b: ThreadId,
       pub access_a: AccessRecord,
       pub access_b: AccessRecord,
   }
   ```

6. Integrate with CORuntime: add an `OwnershipTracker` field, call acquire/release
   around compiled region execution.

7. Add `pub mod ownership;` to /home/z/my-project/vuma/src/cor/src/lib.rs

8. Minimum 15 tests: read/write acquisition, contention, data race detection, deadlock detection.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-cor

Append your work to /home/z/my-project/worklog.md with Task ID W7-T2.
```

### W7-T3: Expand COR Bridge + Integration Tests

```
Task ID: W7-T3
Agent: full-stack-developer

You are expanding the COR bridge module and adding SCG round-trip integration tests.

Files to modify:
1. /home/z/my-project/vuma/src/cor/src/bridge.rs (currently 390 lines — expand to ~800+)
2. /home/z/my-project/vuma/src/tests/src/e2e_cor.rs (currently 5 tests — add 10+ more)

Bridge expansion:
1. Add `bridge_optimize(scg: &mut SCG, profile: &ProfileData) -> Vec<OptimizationSuggestion>`:
   - Use profile data to identify hot paths
   - Suggest inlining for frequently-called small functions
   - Suggest LICM for loop-invariant code in hot loops
   - Suggest devirtualization for monomorphic call sites

2. Add `bridge_verify(scg: &SCG, ive_results: &VerificationSummary) -> Vec<VerificationSuggestion>`:
   - Suggest where to add assertions based on near-miss verification failures
   - Suggest where to add capability annotations for failed exclusivity checks

3. Add `bridge_deploy(scg: &SCG, backends: &[BackendKind]) -> DeploymentPlan`:
   - Choose the best backend for each region based on profile data
   - Schedule compilation for cold regions lazily

Integration tests (in e2e_cor.rs):
1. Test: compile a VUMA function → COR optimizes → re-compile → verify correctness preserved
2. Test: profile-guided inlining changes code but preserves semantics
3. Test: speculative optimization → deoptimization on assumption violation
4. Test: multi-backend deployment (AArch64 + Wasm32 targets)
5. Test: ownership tracking detects a data race in concurrent access
6. Test: bridge_optimize produces valid suggestions from profile data
7. Test: incremental recompilation after SCG delta

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-cor && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W7-T3.
```

### W7-T4: Wave 7 Verification Gate

Same pattern — commit "Wave 7: COR x86_64 JIT, region ownership tracking, expanded bridge+tests"

---

## WAVE 8: Projection — Real SCG Types, AI Conversational, Bidirectional Verification

**Goal**: Make the projection system use real SCG types and produce verified round-trip edits.

**Subagents**: 3

### W8-T1: Replace Placeholder SCG Types with Real Re-exports

```
Task ID: W8-T1
Agent: full-stack-developer

You are replacing the placeholder SCG types in the VUMA projection module with real re-exports from vuma-scg and vuma-bd.

File: /home/z/my-project/vuma/src/projection/src/lib.rs

Current state (lines 52-224): Defines its own NodeId, EdgeId, RegionId, SCGNode, SCGEdge,
SCGRegion, SCG types that duplicate the real ones in vuma_scg.

Step 1: Add vuma-scg and vuma-bd as dependencies to the projection crate's Cargo.toml:
   /home/z/my-project/vuma/src/projection/Cargo.toml

Step 2: Replace all placeholder types with re-exports:
   ```rust
   pub use vuma_scg::{SCG, NodeId, EdgeId, RegionId, SCGNode, SCGEdge, SCGRegion, NodeType, EdgeKind};
   pub use vuma_bd::{BD, BdId, RepD, CapD, RelD};
   ```

Step 3: Update ALL projection modules to use the real types:
   - textual.rs, visual.rs, conversational.rs, diff.rs, bidirectional.rs
   - Search for any code that constructs or matches on the old placeholder types
   - The real SCG types have different field names and structures — update all access patterns

Step 4: Add conversion functions where the projection's internal representation differs from
   the real SCG (e.g., if the projection used simplified edge kinds).

Step 5: Run tests — some projection tests may fail because the real SCG types have different
   constructors. Fix each failing test.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-projection

Append your work to /home/z/my-project/worklog.md with Task ID W8-T1.
```

### W8-T2: AI-Powered Conversational Projection

```
Task ID: W8-T2
Agent: full-stack-developer

You are upgrading the VUMA conversational projection from templated descriptions to AI-powered explanations.

File: /home/z/my-project/vuma/src/projection/src/conversational.rs

Current state (line 1297): "TODO: Replace with LLM-backed suggestion engine."

Implement:

1. Create /home/z/my-project/vuma/src/projection/src/ai_explainer.rs (new file)

2. `AIExplainer` struct that uses the z-ai-web-dev-sdk (backend only):
   ```rust
   pub struct AIExplainer {
       verbosity: Verbosity,
       context_window: usize,
   }
   ```

3. Methods:
   - `explain_node(&self, node: &SCGNode, scg: &SCG) -> String` — natural language description
   - `explain_violation(&self, violation: &VerificationViolation) -> String` — explain why and how to fix
   - `suggest_fix(&self, violation: &VerificationViolation, scg: &SCG) -> Vec<String>` — suggest code changes
   - `summarize_program(&self, scg: &SCG) -> String` — high-level program summary
   - `explain_diff(&self, diff: &SCGDiff) -> String` — explain what changed and why

4. Backend integration — create a Node.js API route in the vuma project that calls z-ai-web-dev-sdk:
   /home/z/my-project/vuma/src/projection/src/ai_backend.rs
   This provides a local HTTP server that the AI explainer calls for LLM completions.

   OR: Use a simpler approach — the AIExplainer builds a prompt and returns it, with an
   optional `call_llm` feature that actually invokes the model. For now, implement the
   prompt-building as the primary path and add a `generate_explanation` method that calls
   the LLM when available.

5. Add `pub mod ai_explainer;` to lib.rs

6. Minimum 8 tests: prompt construction, explanation formatting, violation explanation.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-projection

Append your work to /home/z/my-project/worklog.md with Task ID W8-T2.
```

### W8-T3: Bidirectional Round-Trip Verification Tests

```
Task ID: W8-T3
Agent: full-stack-developer

You are adding round-trip verification tests for the VUMA bidirectional projection.

File: /home/z/my-project/vuma/src/projection/src/bidirectional.rs

Current gap: No tests verify that edit→apply→re-project produces identical output.

Implement:

1. `verify_round_trip(scg: &SCG) -> RoundTripResult`:
   - Project the SCG to textual form
   - Apply the textual projection back to a new SCG
   - Project the new SCG to textual form again
   - Compare the two textual projections — they should be identical (or semantically equivalent)

2. `RoundTripResult`:
   ```rust
   pub struct RoundTripResult {
       pub original_text: String,
       pub roundtrip_text: String,
       pub is_identical: bool,
       pub differences: Vec<ProjectionDifference>,
   }
   pub struct ProjectionDifference {
       pub location: String,
       pub expected: String,
       pub actual: String,
   }
   ```

3. Add 10+ round-trip tests:
   - Simple function: round-trip preserves structure
   - Nested regions: round-trip preserves nesting
   - Concurrent operations: round-trip preserves sync/block structure
   - BD annotations: round-trip preserves behavioral descriptors
   - Error cases: malformed textual input produces meaningful errors

4. Add property-based testing with a SCG generator:
   - Generate random SCGs (within constraints)
   - Verify round-trip for each generated SCG
   - Use a simple strategy: random graph with valid node/edge types

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-projection

Append your work to /home/z/my-project/worklog.md with Task ID W8-T3.
```

### W8-T4: Wave 8 Verification Gate

Same pattern — commit "Wave 8: Real SCG types in projection, AI conversational, bidirectional verification"

---

## WAVE 9: BD Enhancement — RepD::Generic, Expanded Tests, RelD Edge Cases

**Goal**: Make the BD system fully expressive for generic programming and thoroughly tested.

**Subagents**: 3

### W9-T1: Add RepD::Generic Variant

```
Task ID: W9-T1
Agent: full-stack-developer

You are adding a Generic variant to the VUMA RepD enum for proper generic type support.

Files to modify:
1. /home/z/my-project/vuma/src/bd/src/repd.rs — add Generic variant
2. /home/z/my-project/vuma/src/bd/src/repd_compat.rs — handle Generic in compatibility checks
3. /home/z/my-project/vuma/src/bd/src/inference.rs — handle Generic in BD inference
4. /home/z/my-project/vuma/src/bd/src/unify.rs — handle Generic in unification
5. Any file that matches on RepD variants — add Generic case

Step 1: Add variant to RepD enum:
   ```rust
   /// A generic type parameter with a name and optional BD constraints.
   Generic {
       name: String,
       constraints: Vec<BDConstraint>,
   }
   ```
   Where `BDConstraint` is:
   ```rust
   pub enum BDConstraint {
       CapDAtLeast(CapD),       // must have at least these capabilities
       RepDCompatibleWith(RepD), // representation must be compatible with this
       RelDContains(RelD),       // must have these relational constraints
   }
   ```

Step 2: Update `RepD::size()` and `RepD::alignment()` for Generic:
   - Return 0 for size/alignment (unknown until instantiated)
   - Or return a minimum based on constraints

Step 3: Update `repd_compat.rs::compatible()`:
   - Generic is compatible with any RepD that satisfies its constraints
   - Two Generics are compatible if their constraints are satisfiable together

Step 4: Update `unify.rs`:
   - Generic can be unified with any RepD (substitution)
   - Track the substitution in the unification context

Step 5: Update inference.rs:
   - When encountering a generic type parameter, create a Generic RepD
   - When BD inference encounters a concrete type, substitute the Generic

Step 6: Minimum 15 new tests: Generic creation, compatibility, unification, constraint satisfaction.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-bd

Append your work to /home/z/my-project/worklog.md with Task ID W9-T1.
```

### W9-T2: Expand RepD and RelD Test Coverage

```
Task ID: W9-T2
Agent: full-stack-developer

You are expanding test coverage for the VUMA BD RepD and RelD modules, which are currently undertested.

Files to modify:
1. /home/z/my-project/vuma/src/bd/src/repd.rs — add 30+ tests (currently only 6)
2. /home/z/my-project/vuma/src/bd/src/reld.rs — add 15+ tests (currently only 5)
3. /home/z/my-project/vuma/src/bd/src/reld_refine.rs — add 10+ tests for edge cases

RepD tests to add:
1. Struct with nested fields — size/alignment computation
2. Array with various element types and sizes
3. Enum with multiple variants — size is max variant
4. Ptr size/alignment on 64-bit targets
5. Union size is max variant
6. Func size/alignment
7. RepD serialization round-trip
8. Edge cases: zero-size types, max alignment, deeply nested structs

RelD tests to add:
1. Outlives composition: A outlives B, B outlives C → A outlives C
2. BorrowsFrom transitivity
3. DependsOn cycles (should be detected)
4. ContainedIn hierarchy
5. AliasOf / AntiAlias mutual exclusion
6. Secrecy relation propagation
7. Temporal ordering: Succeeds constraints
8. Contradictory constraints: A outlives B AND B outlives A (should detect cycle)

RelD refine tests to add:
1. Refinement with contradictory temporal constraints
2. Composition of Outlives chains
3. Refinement that adds new relations
4. Refinement that conflicts with existing relations
5. Consistency checking for complex relation sets

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-bd

Append your work to /home/z/my-project/worklog.md with Task ID W9-T2.
```

### W9-T3: Wave 9 Verification Gate

Same pattern — commit "Wave 9: RepD::Generic, expanded BD test coverage"

---

## WAVE 10: Execution Validation — Spec Requirement Tests

**Goal**: Satisfy the spec requirements: x86_64 native execution, ARM64 no regression, QEMU execution, Wasm wasmtime validation.

**Subagents**: 5

### W10-T1: x86_64 Native Execution Test

```
Task ID: W10-T1
Agent: full-stack-developer

You are adding an x86_64 native execution test that compiles a VUMA program to x86_64 machine code and executes it natively.

File: /home/z/my-project/vuma/src/tests/src/x86_64_exec.rs (new file)

Implement:

1. `compile_to_x86_64(source: &str) -> Vec<u8>` — compile VUMA source through the full pipeline, targeting x86_64, returning raw machine code bytes.

2. `execute_native(code: &[u8]) -> i64` — mmap the code as RWX, call it, return the result.

3. Tests:
   - `test_x86_64_trivial_return()` — compile `fn main() -> i64 { 42 }`, execute, assert result == 42
   - `test_x86_64_addition()` — compile `fn main() -> i64 { 10 + 32 }`, assert 42
   - `test_x86_64_function_call()` — compile a function that calls another and returns the result
   - `test_x86_64_stack_allocation()` — compile a function with allocate/free, verify it doesn't crash
   - `test_x86_64_conditional()` — compile a function with if/else, verify correct branch taken

Use the COR's execute_code pattern (mmap + mprotect + transmute) but in test code.

Mark tests with `#[cfg(target_arch = "x86_64")]` so they only run on x86_64 hosts.

Add `pub mod x86_64_exec;` to /home/z/my-project/vuma/src/tests/src/lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W10-T1.
```

### W10-T2: ARM64 Non-Regression Test Suite

```
Task ID: W10-T2
Agent: full-stack-developer

You are adding an ARM64 non-regression test suite to ensure the AArch64 backend doesn't regress.

File: /home/z/my-project/vuma/src/tests/src/arm64_regression.rs (new file)

Implement:

1. `compile_to_aarch64(source: &str) -> Vec<u8>` — compile through the pipeline targeting AArch64, returning ELF bytes.

2. `validate_elf_aarch64(elf: &[u8])` — validate:
   - ELF magic (0x7f ELF)
   - Machine type = EM_AARCH64 (183)
   - Type = ET_EXEC (2) or ET_REL (1)
   - Entry point is within the code segment
   - Code segment contains valid AArch64 instructions (check first few words decode properly)

3. Tests:
   - `test_aarch64_trivial_elf()` — compile trivial program, validate ELF structure
   - `test_aarch64_addition_elf()` — compile addition, validate ELF + disassemble code segment
   - `test_aarch64_function_call_elf()` — compile with function call, validate call instruction in disassembly
   - `test_aarch64_stack_frame_elf()` — compile with allocate, validate STP/LDP prologue/epilogue
   - `test_aarch64_conditional_elf()` — compile with if/else, validate B.cond instruction in disassembly
   - `test_aarch64_loop_elf()` — compile with loop, validate backward branch in disassembly

4. Use the AArch64 disassembler (decode_aarch64) to verify instruction patterns.

Add `pub mod arm64_regression;` to lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W10-T2.
```

### W10-T3: Wasm wasmtime Validation Test

```
Task ID: W10-T3
Agent: full-stack-developer

You are adding Wasm wasmtime validation tests to the VUMA test suite.

File: /home/z/my-project/vuma/src/tests/src/wasm_validation.rs (new file)

Implement:

1. `compile_to_wasm(source: &str) -> Vec<u8>` — compile VUMA source through the pipeline targeting Wasm32, returning Wasm binary bytes.

2. `validate_wasm(wasm: &[u8]) -> Result<(), String>` — validate the Wasm binary:
   - Check Wasm magic number (0x00 0x61 0x73 0x6d) and version (0x01 0x00 0x00 0x00)
   - Validate the binary structure (section ordering, size fields)
   - If wasmtime CLI is available, run `wasmtime validate` on the binary

3. `execute_wasm(wasm: &[u8]) -> Result<i64, String>` — if wasmtime is available:
   - Write the wasm to a temp file
   - Run `wasmtime run temp.wasm` and capture output
   - Parse the exit code or output as the result

4. Tests:
   - `test_wasm_trivial_module()` — compile trivial program, validate Wasm structure
   - `test_wasm_addition()` — compile addition, validate + optionally execute
   - `test_wasm_function_call()` — compile with function call, validate
   - `test_wasm_memory_operations()` — compile with load/store, validate
   - `test_wasm_wasmtime_validation()` — if wasmtime available, run full validation (skip otherwise)

Use `#[cfg(target_os = "linux")]` and check for wasmtime availability at test time.

Add `pub mod wasm_validation;` to lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W10-T3.
```

### W10-T4: QEMU Execution Smoke Tests

```
Task ID: W10-T4
Agent: full-stack-developer

You are adding QEMU execution smoke tests for AArch64, x86_64, and RISC-V64 targets.

File: /home/z/my-project/vuma/src/tests/src/qemu_exec.rs (new file)

These tests compile a VUMA program to an ELF binary, then run it in QEMU and verify the exit code or output.

Implement:

1. Helper function: `run_in_qemu(elf: &[u8], machine: &str, qemu_cmd: &str) -> Result<String, String>`
   - Write ELF to a temp file
   - Run `qemu-system-{arch} -M {machine} -kernel {temp} -nographic -no-reboot` with timeout
   - Capture serial output
   - Return the output

2. Tests (skip if QEMU not available):
   - `test_qemu_aarch64_blink()` — compile minimal AArch64 program, run in `qemu-system-aarch64 -M raspi4b`
   - `test_qemu_riscv64_hello()` — compile RISC-V64 program, run in `qemu-system-riscv64 -M virt`
   - `test_qemu_x86_64_hello()` — compile x86_64 program, run in `qemu-system-x86_64`
   - Each test verifies the program doesn't crash (check for "Exception" or "Fault" in output)

3. Add a `QEMU_AVAILABLE` check:
   ```rust
   fn qemu_available(cmd: &str) -> bool {
       std::process::Command::new(cmd)
           .arg("--version")
           .output()
           .is_ok()
   }
   ```

4. Mark all tests as `#[ignore]` by default (QEMU may not be installed in CI).
   They can be run with `cargo test -- --ignored`.

Add `pub mod qemu_exec;` to lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W10-T4.
```

### W10-T5: Wave 10 Verification Gate

Same pattern — commit "Wave 10: Execution validation — x86_64 native, ARM64 regression, Wasm wasmtime, QEMU smoke"

---

## WAVE 11: End-to-End Pipeline — Full Compile→Execute→Verify

**Goal**: Make the full VUMA pipeline work end-to-end: VUMA source → compile → execute → verify output.

**Subagents**: 3

### W11-T1: Full End-to-End Pipeline Test

```
Task ID: W11-T1
Agent: full-stack-developer

You are adding the ultimate end-to-end pipeline test: compile a VUMA program from source, execute the binary, and verify the output.

File: /home/z/my-project/vuma/src/tests/src/e2e_pipeline.rs (new file)

Implement:

1. `compile_and_execute(source: &str) -> Result<ExecutionResult, PipelineError>`:
   - Parse the VUMA source
   - Run through the full 11-stage pipeline (parse → BD → SCG → IVE → proof → codegen → emit)
   - Target x86_64 (since we're on an x86_64 host)
   - Write the ELF to a temp file
   - Execute it as a subprocess
   - Capture stdout, stderr, and exit code

2. `ExecutionResult`:
   ```rust
   pub struct ExecutionResult {
       pub exit_code: i32,
       pub stdout: String,
       pub stderr: String,
       pub compilation_time_ms: u64,
       pub verification_passed: bool,
   }
   ```

3. Tests:
   - `test_e2e_trivial()` — `fn main() { }` → exit code 0
   - `test_e2e_return_value()` — `fn main() -> i64 { 42 }` → exit code 42
   - `test_e2e_arithmetic()` — `fn main() -> i64 { (10 + 20) * 2 }` → exit code 60
   - `test_e2e_region_allocation()` — program with region allocate/free → doesn't crash
   - `test_e2e_verification_passes()` — safe program → IVE passes all 5 invariants
   - `test_e2e_verification_catches_leak()` — program with leaked region → IVE catches it
   - `test_e2e_verification_catches_double_free()` — program with double free → IVE catches it
   - `test_e2e_verification_catches_data_race()` — program with concurrent access → IVE catches it

4. For tests that can't actually execute (e.g., no linker), validate the ELF structure instead.

Add `pub mod e2e_pipeline;` to lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W11-T1.
```

### W11-T2: Pipeline Codegen Stage Fix (Currently Skipped)

```
Task ID: W11-T2
Agent: full-stack-developer

You are fixing the pipeline codegen stage that is currently marked "Skipped" in test pipelines.

File: /home/z/my-project/vuma/src/vuma/src/pipeline.rs

Search for "Skipped" or "skip" in the codegen stage of the pipeline.
The codegen stage should:
1. Lower the SCG to IR (using scg_to_ir)
2. Run register allocation (using regalloc)
3. Run optimization passes (using opt)
4. Emit machine code (using the selected Backend)
5. Produce an ELF binary (using emit)

Debug why the codegen stage is being skipped and fix it:
- It may be that the IR lowering produces invalid IR
- It may be that register allocation fails on the generated IR
- It may be that the Backend returns an error

Fix each issue until the full pipeline can compile a simple VUMA program end-to-end.

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-core

Append your work to /home/z/my-project/worklog.md with Task ID W11-T2.
```

### W11-T3: Wave 11 Verification Gate

Same pattern — commit "Wave 11: End-to-end pipeline, codegen stage fix"

---

## WAVE 12: Final Polish — CI/CD, Performance Regression, Comprehensive Docs

**Goal**: Add CI/CD pipeline, performance regression tests, and ensure every public API has documentation.

**Subagents**: 4

### W12-T1: GitHub Actions CI/CD Pipeline

```
Task ID: W12-T1
Agent: full-stack-developer

Create a GitHub Actions CI/CD pipeline for the VUMA compiler.

File: /home/z/my-project/vuma/.github/workflows/ci.yml (new file)

Implement:

```yaml
name: VUMA CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: 1

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          toolchain: nightly-2026-03-01
          components: rustfmt, clippy, rust-src
          targets: aarch64-unknown-linux-gnu, aarch64-unknown-none
      - run: cargo check --workspace

  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          toolchain: nightly-2026-03-01
          components: rustfmt, clippy, rust-src
          targets: aarch64-unknown-linux-gnu, aarch64-unknown-none
      - run: cargo test --workspace
      - run: cargo test --workspace -- --ignored # QEMU tests

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          toolchain: nightly-2026-03-01
          components: rustfmt, clippy, rust-src
      - run: cargo clippy --workspace -- -D warnings

  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          toolchain: nightly-2026-03-01
          components: rustfmt
      - run: cargo fmt --all -- --check

  pi5-cross:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with:
          toolchain: nightly-2026-03-01
          targets: aarch64-unknown-none
      - run: cargo build -p vuma-pi5 --target aarch64-unknown-none

  no-todo:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: "! grep -rn 'TODO\\|FIXME\\|todo!\\|unimplemented!' src/ --include='*.rs'"
```

Also create .github/workflows/release.yml for tagged releases:
- Build for x86_64, aarch64, wasm32
- Upload artifacts

Run: verify the YAML is valid syntax.

Append your work to /home/z/my-project/worklog.md with Task ID W12-T1.
```

### W12-T2: Performance Regression Tests

```
Task ID: W12-T2
Agent: full-stack-developer

Add performance regression tests to the VUMA test suite.

File: /home/z/my-project/vuma/src/tests/src/perf_regression.rs (new file)

Implement:

1. `BenchmarkCase` struct:
   - name: String
   - source: &str (VUMA program)
   - max_compile_time_ms: u64
   - max_verification_time_ms: u64

2. Benchmark cases:
   - trivial: 1 function, no allocations (compile < 100ms, verify < 50ms)
   - small: 5 functions, 3 regions (compile < 500ms, verify < 200ms)
   - medium: 20 functions, 10 regions, some nesting (compile < 2000ms, verify < 1000ms)
   - concurrent: 5 functions with sync blocks (compile < 1000ms, verify < 500ms)

3. `test_perf_{name}()` for each case — measure compilation and verification time,
   assert it's within the threshold. Use `std::time::Instant::now()` for measurement.

4. Also benchmark individual components:
   - Parser: parse time for 1000-line VUMA source
   - BD inference: inference time for 100-node SCG
   - IVE verification: verification time for 50-node SCG
   - Codegen: code generation time for 20-function IR

5. Store benchmark results in a JSON file for trend tracking.

Add `pub mod perf_regression;` to lib.rs

Run tests: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo test -p vuma-tests

Append your work to /home/z/my-project/worklog.md with Task ID W12-T2.
```

### W12-T3: Comprehensive Doc Comments for All Public APIs

```
Task ID: W12-T3
Agent: full-stack-developer

Add comprehensive doc comments to all public APIs in the VUMA workspace that are missing them.

Steps:

1. Run: source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && cargo doc --workspace --no-deps 2>&1 | grep "warning: missing documentation"
   This will list all public items without doc comments.

2. For each missing doc comment, add a proper Rust doc comment:
   - `/// Brief description of what the item does`
   - `///`
   - `/// # Arguments` (for functions)
   - `/// * `arg_name` - description`
   - `///`
   - `/// # Returns` (for functions with return values)
   - `/// Description of return value`
   - `///`
   - `/// # Example` (where appropriate)
   - `/// ` ` `rust`
   - `/// example code`
   - `/// ` ` `

3. Focus on the most important public APIs first:
   - codegen: Backend trait, TargetInfo trait, BackendKind, emit functions
   - parser: parse_program, Parser struct, ParseError
   - bd: BD, RepD, CapD, RelD, inference functions
   - scg: SCG struct, add_node, add_edge, SCGRegion
   - ive: VerificationEngine, VerificationSummary
   - proof: Proof, Judgment, ProofChecker
   - cor: CORuntime
   - std: All public types

4. After adding all doc comments, run:
   cargo doc --workspace --no-deps
   Verify: zero "missing documentation" warnings.

Append your work to /home/z/my-project/worklog.md with Task ID W12-T3.
```

### W12-T4: Final 100% Verification Gate

```
Task ID: W12-T4
Agent: general-purpose

This is the FINAL verification gate for the entire 91% → 100% effort.

cd /home/z/my-project/vuma && source "$HOME/.cargo/env"

Run EVERY check:

1. cargo check --workspace — ZERO errors, ZERO warnings
2. cargo clippy --workspace — ZERO warnings
3. cargo test --workspace — ZERO failures, ALL 3000+ tests passing
4. cargo fmt --all -- --check — ZERO formatting issues
5. cargo doc --workspace --no-deps — ZERO missing doc warnings
6. rg "TODO|FIXME|todo!|unimplemented!" --type rust src/ — ZERO results
7. rg "STUB|stub|placeholder|hardcoded" --type rust src/ — ZERO results (or all justified)

8. Spec requirements:
   - ARM64 must not regress: cargo test -p vuma-tests arm64_regression
   - x86_64 must execute natively: cargo test -p vuma-tests x86_64_exec (on x86_64 host)
   - QEMU targets must execute: cargo test -p vuma-tests qemu_exec -- --ignored (if QEMU available)
   - Wasm must validate in wasmtime: cargo test -p vuma-tests wasm_validation

9. Git commit and push:
   git add -A
   git commit -m "Wave 12: Final polish — CI/CD, perf regression, comprehensive docs — 100% production ready"
   git push origin main

Report the final score with the same methodology as the initial evaluation.
Expected: 100/100 across all modules.

Append final results to /home/z/my-project/worklog.md with Task ID W12-T4.
```

---

## Summary: Wave Dependency Graph

```
W0 (Trivial Fixes)
 │
 ├── W1 (ELF Relocations) ────────────── independent
 ├── W2 (Stdlib Modules) ─────────────── independent
 ├── W3 (Pi5 BCM2712) ────────────────── independent
 ├── W5 (Proof Enhancement) ──────────── independent
 ├── W6 (SCG Enhancement) ────────────── independent
 ├── W8 (Projection) ──────────────────── depends on W6 (needs real SCG types)
 │
 ├── W4 (IVE Hardening) ──────────────── depends on W6 (needs region inference)
 ├── W9 (BD Enhancement) ─────────────── independent
 │
 ├── W7 (COR Hardening) ──────────────── depends on W4 (needs ownership tracking)
 │
 ├── W10 (Execution Validation) ──────── depends on W1, W6, W7
 ├── W11 (Full Pipeline E2E) ─────────── depends on W10
 │
 └── W12 (Final Polish) ─────────────── depends on ALL previous waves
```

## Parallelism Analysis

| Wave | Subagents | Depends On | Estimated New Lines |
|------|-----------|------------|---------------------|
| W0 | 8 | — | ~500 |
| W1 | 8 | W0 | ~1,200 |
| W2 | 7 | W0 | ~4,000 |
| W3 | 6 | W0 | ~2,500 |
| W4 | 5 | W0+W6 | ~1,500 |
| W5 | 5 | W0 | ~2,000 |
| W6 | 4 | W0 | ~2,000 |
| W7 | 4 | W0+W4 | ~1,800 |
| W8 | 4 | W0+W6 | ~1,500 |
| W9 | 3 | W0 | ~1,200 |
| W10 | 5 | W1+W6+W7 | ~1,200 |
| W11 | 3 | W10 | ~800 |
| W12 | 4 | ALL | ~800 |
| **Total** | **66** | | **~21,000** |

## Expected Final Scores After All Waves

| Module | Current | Target |
|--------|---------|--------|
| Parser | 97 | 100 |
| BD | 95 | 100 |
| SCG | 90 | 100 |
| IVE | 93 | 100 |
| Proof | 88 | 100 |
| Codegen | 89 | 100 |
| COR | 75 | 100 |
| Stdlib | 80 | 100 |
| Pi5 | 70 | 100 |
| Projection | 78 | 100 |
| Core Pipeline | 85 | 100 |
| **Overall** | **91** | **100** |
