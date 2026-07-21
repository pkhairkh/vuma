//! Dump IR at each optimization stage
use vuma_codegen::backend::BackendKind;
use vuma_codegen::ir::IRProgram;
use vuma_codegen::opt;
use vuma_codegen::target_desc::LatencyTable;
use vuma::pipeline::{CompileConfig, CompileTarget, OptLevel, VerificationLevel, bridge_ast_to_codegen_scg};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{ModuleResolver};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let source = std::fs::read_to_string(&path).unwrap();
    let file_path = std::path::Path::new(&path);
    let mut resolver = ModuleResolver::new();
    let ast = resolver.resolve_source(&source, Some(file_path)).unwrap();
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let mut ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).unwrap() };
    // Run IPC lowering
    for func in &mut ir_program.functions {
        vuma_codegen::ipc_lowering::lower_ipc_builtins(func);
    }
    println!("=== AFTER IPC LOWERING ===");
    for func in &ir_program.functions {
        println!("\n--- Function: {} ---", func.name);
        for (i, bb) in func.blocks.iter().enumerate() {
            println!("  bb{}: {}", i, bb.label);
            for instr in &bb.instructions {
                println!("    {:?}", instr);
            }
        }
    }
    // Run DSE only
    let latency_table = LatencyTable::default_ooo();
    let func_map: std::collections::HashMap<String, vuma_codegen::ir::IRFunction> = ir_program.functions.iter().map(|f| (f.name.clone(), f.clone())).collect();
    let func_refs: std::collections::HashMap<String, &vuma_codegen::ir::IRFunction> = func_map.iter().map(|(k, v)| (k.clone(), v)).collect();
    for i in 0..ir_program.functions.len() {
        let f = std::mem::replace(&mut ir_program.functions[i], vuma_codegen::ir::IRFunction::new("__tmp__"));
        let f = opt::constant_fold(f);
        let f = opt::cse(f);
        let (f, provenance) = opt::mark_ive_proven_nonaliasing(f);
        println!("\n=== BEFORE DSE (function {}) ===", ir_program.functions[i].name);
        for (bi, bb) in f.blocks.iter().enumerate() {
            println!("  bb{}: {}", bi, bb.label);
            for instr in &bb.instructions {
                println!("    {:?}", instr);
            }
        }
        let f = opt::dead_store_eliminate(f, &provenance);
        println!("\n=== AFTER DSE (function {}) ===", ir_program.functions[i].name);
        for (bi, bb) in f.blocks.iter().enumerate() {
            println!("  bb{}: {}", bi, bb.label);
            for instr in &bb.instructions {
                println!("    {:?}", instr);
            }
        }
        let _ = func_refs;
        let _ = latency_table;
        ir_program.functions[i] = f;
        break; // only first function
    }
}
