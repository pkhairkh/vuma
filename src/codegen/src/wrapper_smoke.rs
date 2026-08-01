//! # Smoke test — 4 thin-wrapper backends (Task 7-d)
//!
//! Compiles a minimal IR program on each of the 4 thin-wrapper backends
//! (`armeb`, `aarch64_be`, `mips64be`, `ppc64le`) and verifies that:
//!   1. `allocate_registers` does not panic,
//!   2. `encode_function` produces non-empty output (≥4 bytes — at least
//!      one instruction word).
//!
//! This is a *smoke* test — it does NOT exercise QEMU. It only verifies
//! that the wrapper's `allocate_registers` + `encode_function` path works
//! end-to-end on a trivial program (a single empty block with
//! `IRTerminator::Return(vec![])`). This deliberately avoids
//! `IRInstr::Syscall` so that the `unimplemented!()` panic in the
//! `mips64` and `ppc64` parents does not affect this test. See the
//! per-wrapper `test_syscall_inherited_from_*` tests in each backend
//! file for the -pending case.
//!
//! Wired into `vuma-codegen`'s test build via
//! `#[cfg(test)] mod wrapper_smoke;` in `lib.rs`.

#![cfg(test)]

use crate::aarch64_be::AArch64BeBackend;
use crate::armeb::ArmEbBackend;
use crate::backend::Backend;
use crate::ir::{IRBlock, IRFunction, IRTerminator};
use crate::mips64be::Mips64BeBackend;
use crate::ppc64le::PPC64LEBackend;

use std::collections::HashSet;

/// Build the trivial "smoke" function: a single empty `entry` block with
/// terminator `Return(vec![])` (no return values, no instructions, no
/// syscalls). This avoids `IRInstr::Syscall` entirely so the
/// panic in `mips64` / `ppc64` parents does not fire.
fn build_smoke_function() -> IRFunction {
    IRFunction {
        name: "smoke".to_string(),
        params: vec![],
        results: vec![],
        param_types: vec![],
        result_types: vec![],
        vregs: std::collections::HashMap::new(),
        blocks: vec![IRBlock {
            label: "entry".to_string(),
            instructions: vec![],
            terminator: IRTerminator::Return(vec![]),
            predecessors: HashSet::new(),
            successors: HashSet::new(),
            source_line: 0,
        }],
        source_file: String::new(),
    }
}

/// Run the smoke pipeline on one backend: `allocate_registers` then
/// `encode_function`. Returns the encoded byte count. Panics if either
/// step fails or if the encoded output is empty.
fn run_smoke<B: Backend>(backend: &B, name: &str) -> usize {
    let func = build_smoke_function();
    let allocated = backend
        .allocate_registers(&func)
        .unwrap_or_else(|e| panic!("smoke[{name}]: allocate_registers failed: {e:?}"));
    let bytes = backend
        .encode_function(&allocated)
        .unwrap_or_else(|e| panic!("smoke[{name}]: encode_function failed: {e:?}"));
    assert!(
        !bytes.is_empty(),
        "smoke[{name}]: encode_function returned empty output"
    );
    bytes.len()
}

/// Smoke test for the 4 thin-wrapper backends. Verifies that each wrapper
/// can compile a trivial empty function (no Syscall) and produce non-empty
/// encoded output (at least one 4-byte instruction word — the function
/// epilogue / return sequence). This deliberately avoids
/// `IRInstr::Syscall` so the `unimplemented!()` panic in
/// `mips64` / `ppc64` parents does not fire here.
///
/// The per-wrapper `test_syscall_inherited_from_*` tests in each
/// backend file (e.g. `armeb.rs::tests::test_syscall_inherited_from_arm32`)
/// exercise the -pending Syscall path with `catch_unwind`.
#[test]
fn test_smoke_compile_simple_program_all_4_wrappers() {
    let names_and_lengths: Vec<(&str, usize)> = vec![
        ("armeb", run_smoke(&ArmEbBackend::new(), "armeb")),
        (
            "aarch64_be",
            run_smoke(&AArch64BeBackend::new(), "aarch64_be"),
        ),
        ("mips64be", run_smoke(&Mips64BeBackend::new(), "mips64be")),
        ("ppc64le", run_smoke(&PPC64LEBackend::new(), "ppc64le")),
    ];

    eprintln!("\n════════ Task 7-d smoke test: 4 thin-wrapper backends ════════");
    eprintln!("  {:<14} {:<10}", "Wrapper", "Bytes");
    eprintln!("  {}", "-".repeat(28));
    for (name, n) in &names_and_lengths {
        eprintln!("  {:<14} {:<10}", name, n);
    }
    eprintln!();

    // Sanity: each wrapper must emit at least one 4-byte instruction word
    // (the function epilogue / return sequence). The exact byte count
    // varies per backend but should always be ≥ 4 bytes.
    for (name, n) in &names_and_lengths {
        assert!(
            *n >= 4,
            "smoke[{name}]: expected ≥4 bytes (one instruction word), got {n}"
        );
    }
}
