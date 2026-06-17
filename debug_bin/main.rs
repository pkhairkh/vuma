// Quick debug: print IR for `fn main() -> i32 { return 79; }`
fn main() {
    let source = "fn main() -> i32 { return 79; }";
    let mut parser = vuma_parser::Parser::new(source);
    let po = parser.parse_program();
    if po.has_errors() { eprintln!("parse errors: {:?}", po.errors); return; }
    let ast = po.unwrap();
    let mut conv = vuma_parser::AstToScg::new();
    let scg = conv.convert(&ast).expect("ast->scg");
    eprintln!("SCG nodes: {}", scg.node_count());
    let codegen_scg = vuma::pipeline::bridge_scg_to_codegen(&scg);
    let mut builder = vuma_codegen::scg_to_ir::IRBuilder::new();
    let ir = builder.build(&codegen_scg).expect("ir build");
    for f in &ir.functions {
        eprintln!("=== IR function: {:?} params={:?} results={:?}", f.name, f.params, f.results);
        for (i, b) in f.blocks.iter().enumerate() {
            eprintln!("  block[{}] {:?}:", i, b.label);
            for instr in &b.instructions {
                eprintln!("    {:?}", instr);
            }
            eprintln!("    terminator: {:?}", b.terminator);
        }
    }
    // Encode via x86_64
    let backend = vuma_codegen::backend::create_backend(vuma_codegen::backend::BackendKind::X86_64).unwrap();
    let mut alloc = Vec::new();
    for f in &ir.functions {
        let a = backend.allocate_registers(f).expect("regalloc");
        eprintln!("=== Allocated function: {:?} code_size={}", a.name, a.code_size);
        for b in &a.blocks {
            for instr in &b.instructions {
                eprintln!("    {} reads={:?} writes={:?}", instr.opcode, instr.reads, instr.writes);
            }
        }
        alloc.push(a);
    }
    let total: usize = alloc.iter().map(|f| f.code_size).sum();
    let prog = vuma_codegen::backend::AllocatedProgram { functions: alloc, total_code_size: total, total_data_size: 0 };
    let elf = backend.encode_program(&prog).expect("encode");
    eprintln!("ELF size: {}", elf.len());
    std::fs::write("/tmp/vuma_debug_x86_64.elf", &elf).unwrap();
}
