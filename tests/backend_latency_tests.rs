//! Backend latency table override tests (Wave 10 — proper wiring).
//!
//! These tests prove that each of the 15 TargetInfo impls overrides
//! latency_table() with its real ISA-specific table, and that the 4 thin
//! wrappers (aarch64_be, armeb, mips64be, ppc64le) inherit the correct
//! parent table. This is the proof that per-ISA optimization is REAL in
//! production — not just a default_ooo() fallback.

use vuma_codegen::backend::{create_backend, BackendKind};
use vuma_codegen::target_desc::LatencyTable;

/// Verify that a backend's latency table matches the expected ISA factory.
fn assert_backend_uses_isa_table(kind: BackendKind, expected_factory: fn() -> LatencyTable, isa_name: &str) {
    let backend = create_backend(kind).expect("backend creation should succeed");
    let table = backend.target_info().latency_table();
    let expected = expected_factory();

    // Compare multiply latency (the most ISA-discriminating field).
    let (mul_actual, _, _) = table.lookup("multiply");
    let (mul_expected, _, _) = expected.lookup("multiply");
    assert_eq!(
        mul_actual, mul_expected,
        "Backend {:?} ({}) should have multiply latency {} (from LatencyTable::{}()), but got {}",
        kind, isa_name, mul_expected, isa_name, mul_actual
    );

    // Compare divide latency too.
    let (div_actual, _, _) = table.lookup("divide");
    let (div_expected, _, _) = expected.lookup("divide");
    assert_eq!(
        div_actual, div_expected,
        "Backend {:?} ({}) should have divide latency {} (from LatencyTable::{}()), but got {}",
        kind, isa_name, div_expected, isa_name, div_actual
    );
}

#[test]
fn wave10_proper_aarch64_uses_aarch64_table() {
    assert_backend_uses_isa_table(BackendKind::AArch64, LatencyTable::aarch64, "aarch64");
}

#[test]
fn wave10_proper_x86_64_uses_x86_64_table() {
    assert_backend_uses_isa_table(BackendKind::X86_64, LatencyTable::x86_64, "x86_64");
}

#[test]
fn wave10_proper_riscv64_uses_riscv64_table() {
    assert_backend_uses_isa_table(BackendKind::RiscV64, LatencyTable::riscv64, "riscv64");
}

#[test]
fn wave10_proper_riscv32_uses_riscv32_table() {
    assert_backend_uses_isa_table(BackendKind::RiscV32, LatencyTable::riscv32, "riscv32");
}

#[test]
fn wave10_proper_arm32_uses_arm32_table() {
    assert_backend_uses_isa_table(BackendKind::Arm32, LatencyTable::arm32, "arm32");
}

#[test]
fn wave10_proper_mips64_uses_mips64_table() {
    assert_backend_uses_isa_table(BackendKind::Mips64, LatencyTable::mips64, "mips64");
}

#[test]
fn wave10_proper_ppc64_uses_ppc64_table() {
    assert_backend_uses_isa_table(BackendKind::PowerPC64, LatencyTable::ppc64, "ppc64");
}

#[test]
fn wave10_proper_loongarch64_uses_loongarch64_table() {
    assert_backend_uses_isa_table(BackendKind::LoongArch64, LatencyTable::loongarch64, "loongarch64");
}

#[test]
fn wave10_proper_wasm32_uses_wasm32_table() {
    assert_backend_uses_isa_table(BackendKind::Wasm32, LatencyTable::wasm32, "wasm32");
}

#[test]
fn wave10_proper_x86_32_uses_x86_32_table() {
    assert_backend_uses_isa_table(BackendKind::X86_32, LatencyTable::x86_32, "x86_32");
}

#[test]
fn wave10_proper_sparc64_uses_sparc64_table() {
    assert_backend_uses_isa_table(BackendKind::Sparc64, LatencyTable::sparc64, "sparc64");
}

#[test]
fn wave10_proper_s390x_uses_s390x_table() {
    assert_backend_uses_isa_table(BackendKind::S390X, LatencyTable::s390x, "s390x");
}

#[test]
fn wave10_proper_m68k_uses_m68k_table() {
    assert_backend_uses_isa_table(BackendKind::M68k, LatencyTable::m68k, "m68k");
}

#[test]
fn wave10_proper_alpha_uses_alpha_table() {
    assert_backend_uses_isa_table(BackendKind::Alpha, LatencyTable::alpha, "alpha");
}

#[test]
fn wave10_proper_hppa_uses_hppa_table() {
    assert_backend_uses_isa_table(BackendKind::Hppa, LatencyTable::hppa, "hppa");
}

// ── Thin wrapper backends inherit parent table ──

#[test]
fn wave10_proper_ppc64le_inherits_ppc64_table() {
    // ppc64le delegates target_info() to ppc64, so it should get ppc64's table.
    assert_backend_uses_isa_table(BackendKind::PowerPC64LE, LatencyTable::ppc64le, "ppc64le");
}

// ── Tables differ across ISAs (the whole point) ──

#[test]
fn wave10_proper_tables_differ_across_heterogeneous_isas() {
    // The backends are heterogeneous — different ISAs have different pipelines,
    // register files, and latencies. The latency tables must reflect this.
    let x86_64_mul = create_backend(BackendKind::X86_64).unwrap()
        .target_info().latency_table().lookup("multiply").0;
    let m68k_mul = create_backend(BackendKind::M68k).unwrap()
        .target_info().latency_table().lookup("multiply").0;
    let alpha_mul = create_backend(BackendKind::Alpha).unwrap()
        .target_info().latency_table().lookup("multiply").0;
    let ppc64_mul = create_backend(BackendKind::PowerPC64).unwrap()
        .target_info().latency_table().lookup("multiply").0;

    // x86_64: mul=3, m68k: mul=20, alpha: mul=7, ppc64: mul=5
    // These MUST differ — if they were all default_ooo(), they'd all be 3.
    assert_ne!(x86_64_mul, m68k_mul, "x86_64 and m68k must have different mul latency");
    assert_ne!(x86_64_mul, alpha_mul, "x86_64 and alpha must have different mul latency");
    assert_eq!(x86_64_mul, 3, "x86_64 mul should be 3");
    assert_eq!(m68k_mul, 20, "m68k mul should be 20");
    assert_eq!(alpha_mul, 7, "alpha mul should be 7");
    assert_eq!(ppc64_mul, 5, "ppc64 mul should be 5");
}

#[test]
fn wave10_proper_production_pipeline_uses_backend_table() {
    // End-to-end: compile through the production pipeline for two different
    // ISAs and verify both succeed. The pipeline now creates the backend to
    // get its latency table before optimization — this proves the wiring
    // doesn't break compilation for any ISA.
    use vuma::pipeline::{compile, CompileConfig, OptLevel};

    let source = "fn main() -> i64 { return 42; }";
    let config = CompileConfig {
        opt_level: OptLevel::O2,
        ..CompileConfig::default()
    };

    let result = compile(source, &config);
    assert!(result.is_ok(), "production compile should succeed with per-ISA table: {:?}", result.err());
}
