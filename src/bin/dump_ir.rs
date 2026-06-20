//! Dump IR for a .vuma file.
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, bridge_scg_to_codegen};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = if args.len() > 1 { &args[1] } else { "examples/test_call.vuma" };
    let source = std::fs::read_to_string(path).unwrap();
    let mut parser = Parser::new(&source);
    let result = parser.parse_program();
    if result.has_errors() {
        eprintln!("Parse errors: {}", result.errors.len());
        return;
    }
    let ast = result.unwrap();
    let mut scg = { let mut c = AstToScg::new(); c.convert(&ast).map_err(|e| format!("scg: {}", e)).unwrap() };
    let config = CompileConfig { opt_level: vuma::pipeline::OptLevel::O0, ..Default::default() };
    let _ = run_scg_transforms(&mut scg, &config);
    let codegen_scg = bridge_scg_to_codegen(&scg);
    let ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).map_err(|e| format!("ir: {}", e)).unwrap() };
    for func in &ir_program.functions {
        println!("Function: {} ({} params, {} vregs)", func.name, func.params.len(), func.vregs.len());
        for (id, vr) in &func.vregs {
            println!("  vreg {}: {:?}", id, vr);
        }
        for block in &func.blocks {
            println!("  Block: {}", block.label);
            for instr in &block.instructions {
                println!("    {:?}", instr);
            }
            println!("    TERM: {:?}", block.terminator);
        }
    }
}
