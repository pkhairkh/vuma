//! Standalone test binary to compile a .vuma file and dump the resulting AArch64 ELF bytes.
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel, bridge_scg_to_codegen};

fn compile_for_backend(source: &str, kind: BackendKind) -> Result<Vec<u8>, String> {
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
    backend.encode_program(&program).map_err(|e| format!("encode: {}", e))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let out_path = &args[2];
    let source = std::fs::read_to_string(path).unwrap();
    let binary = compile_for_backend(&source, BackendKind::AArch64).unwrap();
    std::fs::write(out_path, &binary).unwrap();
    eprintln!("Wrote {} bytes to {}", binary.len(), out_path);
}
