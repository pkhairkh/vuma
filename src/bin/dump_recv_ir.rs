//! Dump the IR after the full optimization pipeline
use vuma_codegen::backend::BackendKind;
use vuma::pipeline::{CompileConfig, run_ir_pipeline, CompileTarget, OptLevel, VerificationLevel, bridge_ast_to_codegen_scg};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, ModuleResolver};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    let file_path = std::path::Path::new(&path);
    let mut resolver = ModuleResolver::new();
    let ast = resolver.resolve_source(&source, Some(file_path)).unwrap();
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).unwrap() };
    let o3_config = CompileConfig {
        target: CompileTarget::Linux,
        opt_level: OptLevel::O3,
        verification_level: VerificationLevel::Normal,
        inline_threshold: 0,
        ..Default::default()
    };
    let mut timings: Vec<(String, u64)> = Vec::new();
    let ir_program = run_ir_pipeline(ir_program, &o3_config, BackendKind::AArch64, &mut timings).unwrap();
    for func in &ir_program.functions {
        println!("\n--- Function: {} ---", func.name);
        for (i, bb) in func.blocks.iter().enumerate() {
            println!("  bb{}: {}", i, bb.label);
            for instr in &bb.instructions {
                println!("    {:?}", instr);
            }
            println!("    TERM: {:?}", bb.terminator);
        }
    }
}
