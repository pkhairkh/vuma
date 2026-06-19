// Standalone test: compile test_print.vuma for each backend and run it.
use vuma::pipeline::{CompileConfig, OptLevel, VerificationLevel, run_scg_transforms, CompileTarget, bridge_scg_to_codegen};
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use std::process::Command;
use std::fs;

fn compile_per_backend(source: &str, kind: BackendKind) -> Result<Vec<u8>, String> {
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
    let total: usize = allocated.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram { functions: allocated, total_code_size: total, total_data_size: 0 };
    backend.encode_program(&program).map_err(|e| format!("encode: {}", e))
}

fn execute_binary(binary: &[u8], qemu: Option<&str>, timeout_secs: u64) -> (i32, Vec<u8>, Vec<u8>) {
    let bin_path = std::env::temp_dir().join(format!("vuma_exec_{}.bin", std::process::id()));
    let _ = fs::write(&bin_path, binary);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&bin_path, perms);
    }
    let mut cmd = Command::new("timeout");
    cmd.arg(format!("{}", timeout_secs));
    if let Some(q) = qemu { cmd.arg(q); }
    cmd.arg(&bin_path);
    let output = cmd.output();
    let _ = fs::remove_file(&bin_path);
    match output {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            (code, o.stdout, o.stderr)
        }
        Err(_) => (-1, vec![], b"command failed".to_vec()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let example = if args.len() > 1 { args[1].clone() } else { "test_print.vuma".to_string() };
    let source = fs::read_to_string(format!("/tmp/vuma/examples/{}", example)).unwrap();
    println!("=== Source: {} ===\n{}", example, source);

    let backends: &[(&str, BackendKind, Option<&str>)] = &[
        ("x86_64", BackendKind::X86_64, None),
        ("aarch64", BackendKind::AArch64, Some("/tmp/qemu_bins/qemu-aarch64")),
        ("riscv64", BackendKind::RiscV64, Some("/tmp/qemu_bins/qemu-riscv64")),
        ("arm32", BackendKind::Arm32, Some("/tmp/qemu_bins/qemu-arm")),
        ("ppc64", BackendKind::PowerPC64, Some("/tmp/qemu_bins/qemu-ppc64")),
        ("mips64", BackendKind::Mips64, Some("/tmp/qemu_bins/qemu-mips64")),
        ("loongarch64", BackendKind::LoongArch64, Some("/tmp/qemu_bins/qemu-loongarch64")),
    ];

    for (name, kind, qemu) in backends {
        println!("\n=== {} ===", name);
        match compile_per_backend(&source, *kind) {
            Err(e) => println!("COMPILE FAIL: {}", e),
            Ok(binary) => {
                let (code, stdout, stderr) = execute_binary(&binary, *qemu, 2);
                let stdout_s = String::from_utf8_lossy(&stdout);
                let stderr_s = String::from_utf8_lossy(&stderr);
                println!("exit code: {}", code);
                println!("stdout: {:?}", stdout_s);
                if !stderr_s.is_empty() {
                    println!("stderr (first 200): {:?}", &stderr_s[..stderr_s.len().min(200)]);
                }
            }
        }
    }
}
