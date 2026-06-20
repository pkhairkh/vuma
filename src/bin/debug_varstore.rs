//! Debug topological sort + IR for varstore bug.
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, bridge_scg_to_codegen};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = if args.len() > 1 { &args[1] } else { "/tmp/test_varstore.vuma" };
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

    for node in &codegen_scg.nodes {
        if let vuma_codegen::scg_to_ir::ScgNode::Function(func) = node {
            println!("=== SCG statements (source order) ===");
            for (i, stmt) in func.body.iter().enumerate() {
                println!("  [{}]: {:?}", i, stmt);
            }
            println!("\n=== topological_sort_statements order ===");
            let order = IRBuilder::topological_sort_statements(&func.body);
            println!("  order = {:?}", order);
            println!("\n=== Sorted SCG statements ===");
            for &idx in &order {
                println!("  [{}]: {:?}", idx, func.body[idx]);
            }
        }
    }

    // Now build IR and print
    let ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).map_err(|e| format!("ir: {}", e)).unwrap() };
    for func in &ir_program.functions {
        println!("\n=== IR Function: {} ===", func.name);
        for (id, vr) in &func.vregs {
            println!("  vreg {}: {:?}", id, vr);
        }
        for block in &func.blocks {
            println!("  Block: {}", block.label);
            for (i, instr) in block.instructions.iter().enumerate() {
                println!("    [{}]: {:?}", i, instr);
            }
            println!("    TERM: {:?}", block.terminator);
        }
    }
}
