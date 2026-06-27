//! Dump codegen SCG statements for a .vuma file.
use vuma_codegen::scg_to_ir::{ScgStatement, ScgExpr};
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, bridge_scg_to_codegen};

fn dump_stmt(stmt: &ScgStatement, indent: usize) {
    let pad = "  ".repeat(indent);
    match stmt {
        ScgStatement::Computation(c) => {
            println!("{}Computation: dst={} op={:?} lhs={:?} rhs={:?} reassigns={:?}", pad, c.dst, c.op, c.lhs, c.rhs, c.reassigns);
        }
        ScgStatement::Call(c) => {
            println!("{}Call: dst={:?} func={} args={:?} reassigns={:?}", pad, c.dst, c.func, c.args, c.reassigns);
        }
        ScgStatement::Access(a) => {
            println!("{}Access: {:?}", pad, a);
        }
        ScgStatement::Allocation(a) => {
            println!("{}Allocation: {:?}", pad, a);
        }
        ScgStatement::Return(vals) => {
            println!("{}Return: {:?}", pad, vals);
        }
        ScgStatement::Control(c) => {
            println!("{}Control: {:?}", pad, c);
        }
        _ => println!("{}Other: {:?}", pad, stmt),
    }
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "examples/test_call.vuma".to_string());
    let source = std::fs::read_to_string(&path).unwrap();
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
            println!("=== Function: {} ===", func.name);
            for stmt in &func.body {
                dump_stmt(stmt, 0);
            }
            println!();
        }
    }
}
