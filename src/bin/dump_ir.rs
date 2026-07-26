//! Dump the IR for a .vuma file
use vuma::pipeline::bridge_ast_to_codegen_scg;
use vuma_codegen::backend::{create_backend, BackendKind};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::ModuleResolver;

fn backend_from_name(name: &str) -> Result<BackendKind, String> {
    match name.to_ascii_lowercase().as_str() {
        "x86_64" | "x86-64" | "x64" => Ok(BackendKind::X86_64),
        "aarch64" | "arm64" => Ok(BackendKind::AArch64),
        "riscv64" | "riscv" => Ok(BackendKind::RiscV64),
        "riscv32" => Ok(BackendKind::RiscV32),
        "x86_32" | "i386" | "x86" => Ok(BackendKind::X86_32),
        "arm32" | "arm" => Ok(BackendKind::Arm32),
        "mips64" | "mips" => Ok(BackendKind::Mips64),
        "ppc64" | "powerpc64" | "ppc" => Ok(BackendKind::PowerPC64),
        "ppc64le" | "powerpc64le" | "ppcle" => Ok(BackendKind::PowerPC64LE),
        "loongarch64" | "loongarch" => Ok(BackendKind::LoongArch64),
        "wasm32" | "wasm" => Ok(BackendKind::Wasm32),
        "sparc64" | "sparc" => Ok(BackendKind::Sparc64),
        "s390x" | "s390" => Ok(BackendKind::S390X),
        "mips64be" | "mips64-be" => Ok(BackendKind::Mips64Be),
        "armeb" | "arm-be" => Ok(BackendKind::ArmEb),
        "aarch64_be" | "aarch64be" => Ok(BackendKind::AArch64Be),
        "m68k" => Ok(BackendKind::M68k),
        "alpha" => Ok(BackendKind::Alpha),
        "hppa" | "parisc" => Ok(BackendKind::Hppa),
        _ => Err(format!("unknown backend: {}", name)),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let backend_name = if args.len() > 2 {
        args[2].as_str()
    } else {
        "arm32"
    };
    let kind = backend_from_name(backend_name).unwrap_or(BackendKind::Arm32);
    let source = std::fs::read_to_string(path).unwrap();
    let file_path = std::path::Path::new(path);

    let mut resolver = ModuleResolver::new();
    let ast = resolver.resolve_source(&source, Some(file_path)).unwrap();

    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let mut b = IRBuilder::new();
    let ir_program = b.build(&codegen_scg).unwrap();
    let mut ir_program = ir_program;
    for func in &mut ir_program.functions {
        vuma_codegen::ipc_lowering::lower_ipc_builtins(func, kind);
    }
    // Use the full production pipeline (matches compile_dump) so the IR
    // we display reflects all post-lowering opts + backend latency table.
    use vuma::pipeline::{
        run_ir_pipeline, CompileConfig, CompileTarget, OptLevel, VerificationLevel,
    };
    let cfg = CompileConfig {
        target: if kind == vuma_codegen::backend::BackendKind::Wasm32 {
            CompileTarget::Wasm32
        } else {
            CompileTarget::Linux
        },
        opt_level: OptLevel::O3,
        verification_level: VerificationLevel::Normal,
        inline_threshold: 0,
        ..Default::default()
    };
    let mut timings: Vec<(String, u64)> = Vec::new();
    let ir_program = run_ir_pipeline(ir_program, &cfg, kind, &mut timings).unwrap();

    println!("=== IR for {} (backend={}) ===", path, backend_name);
    for func in &ir_program.functions {
        println!(
            "\n--- Function: {} (params={:?} returns={:?}) ---",
            func.name, func.param_types, func.result_types
        );
        for (i, bb) in func.blocks.iter().enumerate() {
            println!(
                "  bb{}: label={:?} preds={:?}",
                i, bb.label, bb.predecessors
            );
            for instr in &bb.instructions {
                println!("    {:?}", instr);
            }
            println!("    TERM: {:?}", bb.terminator);
        }
    }

    let backend = create_backend(kind).unwrap();
    for func in &ir_program.functions {
        let allocated = backend.allocate_registers(func).unwrap();
        println!("\n=== Allocated: {} ===", func.name);
        for (i, bb) in allocated.blocks.iter().enumerate() {
            println!("  bb{}:", i);
            for instr in &bb.instructions {
                println!("    {:?}", instr);
            }
        }
    }
}
// placeholder
