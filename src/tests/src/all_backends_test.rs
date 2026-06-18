
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel, bridge_scg_to_codegen};

fn compile_for_backend(source: &str, kind: BackendKind) -> Result<usize, String> {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    if result.has_errors() { return Err(format!("parse: {} errors", result.errors.len())); }
    let ast = result.unwrap();
    let mut scg = { let mut c = AstToScg::new(); c.convert(&ast).map_err(|e| format!("scg: {}", e))? };
    let config = CompileConfig {
        target: if kind == BackendKind::Wasm32 { CompileTarget::Wasm32 } else { CompileTarget::Linux },
        opt_level: OptLevel::O0, verification_level: VerificationLevel::None, ..Default::default()
    };
    let _ = run_scg_transforms(&mut scg, &config);
    let codegen_scg = bridge_scg_to_codegen(&scg);
    let ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).map_err(|e| format!("ir: {}", e))? };
    let backend = create_backend(kind).map_err(|e| format!("backend: {}", e))?;
    let mut allocated = Vec::new();
    for func in &ir_program.functions {
        allocated.push(backend.allocate_registers(func).map_err(|e| format!("regalloc: {}", e))?);
    }
    let total_code: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram { functions: allocated, total_code_size: total_code, total_data_size: 0 };
    let binary = backend.encode_program(&program).map_err(|e| format!("encode: {}", e))?;
    Ok(binary.len())
}

#[test]
fn test_all_examples_all_backends() {
    let examples_dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut examples: Vec<String> = std::fs::read_dir(&examples_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma")).collect();
    examples.sort();
    
    let backends = [
        (BackendKind::AArch64, "aarch64"), (BackendKind::X86_64, "x86_64"),
        (BackendKind::RiscV64, "riscv64"), (BackendKind::Arm32, "arm32"),
        (BackendKind::Mips64, "mips64"), (BackendKind::PowerPC64, "ppc64"),
        (BackendKind::LoongArch64, "loongarch64"), (BackendKind::Wasm32, "wasm32"),
    ];
    
    let mut results = Vec::new();
    for ex in &examples {
        let source = std::fs::read_to_string(format!("{}/{}", examples_dir, ex)).unwrap();
        for (kind, name) in &backends {
            match compile_for_backend(&source, *kind) {
                Ok(size) => results.push((ex, *name, "PASS", format!("{} bytes", size))),
                Err(e) => results.push((ex, *name, "FAIL", e.chars().take(60).collect())),
            }
        }
    }
    
    let mut by_backend: std::collections::BTreeMap<&str, (usize, usize)> = std::collections::BTreeMap::new();
    for (_, backend, status, _) in &results {
        let e = by_backend.entry(*backend).or_insert((0, 0));
        if *status == "PASS" { e.0 += 1; } else { e.1 += 1; }
    }
    
    eprintln!("\n=== All Examples x All Backends ===");
    eprintln!("{:<25} {:>6} {:>6}", "Backend", "Pass", "Fail");
    for (backend, (pass, fail)) in &by_backend {
        eprintln!("{:<25} {:>6} {:>6}", backend, pass, fail);
    }
    
    eprintln!("\n=== Failures ===");
    for (ex, backend, status, detail) in &results {
        if *status == "FAIL" { eprintln!("  {} {}: {}", ex, backend, detail); }
    }
    
    let total_pass = results.iter().filter(|(_, _, s, _)| *s == "PASS").count();
    eprintln!("\n=== {} / {} combinations pass ===", total_pass, results.len());
}
