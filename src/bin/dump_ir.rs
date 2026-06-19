use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel, bridge_scg_to_codegen};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let source = std::fs::read_to_string(path).unwrap();
    let mut parser = Parser::new(&source);
    let result = parser.parse_program();
    if result.has_errors() { eprintln!("parse errors"); return; }
    let ast = result.unwrap();
    let mut scg = { let mut c = AstToScg::new(); c.convert(&ast).unwrap() };
    let config = CompileConfig { target: CompileTarget::Linux, opt_level: OptLevel::O0, verification_level: VerificationLevel::None, ..Default::default() };
    let _ = run_scg_transforms(&mut scg, &config);
    let codegen_scg = bridge_scg_to_codegen(&scg);
    let ir_program = { let mut b = IRBuilder::new(); b.build(&codegen_scg).unwrap() };
    for func in &ir_program.functions {
        println!("Function: {} ({} params, {} vregs)", func.name, func.params.len(), func.vregs.len());
        for (vid, vr) in &func.vregs {
            println!("  vreg {}: {:?}", vid, vr);
        }
        for block in &func.blocks {
            println!("  Block: {}", block.label);
            for instr in &block.instructions {
                println!("    {:?}", instr);
            }
            println!("    TERM: {:?}", block.terminator);
        }
    }
    let backend = create_backend(BackendKind::LoongArch64).unwrap();
    let mut allocated: Vec<vuma_codegen::backend::AllocatedFunction> = Vec::new();
    for func in &ir_program.functions {
        let a = backend.allocate_registers(func).unwrap();
        println!("\n=== Allocated function {} (frame_size={}, code_size={}) ===", a.name, a.frame_size, a.code_size);
        for block in &a.blocks {
            println!("  Block: {}", block.label);
            for instr in &block.instructions {
                let bytes: Vec<String> = instr.encoded.iter().map(|b| format!("{:02x}", b)).collect();
                println!("    [{}] {}", bytes.join(""), instr.opcode);
            }
        }
    }
}
