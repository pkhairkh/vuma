//! Standalone tool to compile a .vuma file and dump the resulting ELF bytes.
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel, bridge_scg_to_codegen};
use std::process::Command;
use std::fs;

fn backend_from_name(name: &str) -> Result<BackendKind, String> {
    match name.to_ascii_lowercase().as_str() {
        "x86_64" | "x86-64" | "x64" => Ok(BackendKind::X86_64),
        "aarch64" | "arm64" => Ok(BackendKind::AArch64),
        "riscv64" | "riscv" => Ok(BackendKind::RiscV64),
        "arm32" | "arm" => Ok(BackendKind::Arm32),
        "mips64" | "mips" => Ok(BackendKind::Mips64),
        "ppc64" | "powerpc64" | "ppc" => Ok(BackendKind::PowerPC64),
        "loongarch64" | "loongarch" => Ok(BackendKind::LoongArch64),
        "wasm32" | "wasm" => Ok(BackendKind::Wasm32),
        _ => Err(format!("unknown backend: {}", name)),
    }
}

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

fn execute_binary(binary: &[u8], qemu: &str, timeout_secs: u64) -> (i32, Vec<u8>, Vec<u8>, bool) {
    let bin_path = std::env::temp_dir().join(format!("vuma_diag_{}.bin", std::process::id()));
    let _ = fs::write(&bin_path, binary);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&bin_path, perms);
    }
    let output = Command::new("timeout")
        .arg(format!("{}", timeout_secs))
        .arg(qemu)
        .arg(&bin_path)
        .output();
    let _ = fs::remove_file(&bin_path);
    match output {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let crashed = stderr.contains("Segmentation fault")
                || stderr.contains("uncaught target signal")
                || code == 139 || code == 134;
            (code, o.stdout, o.stderr, crashed)
        }
        Err(_) => (-1, vec![], vec![], true),
    }
}

fn run_diag(backend_name: &str, examples_dir: &str, qemu: Option<&str>) {
    let kind = match backend_from_name(backend_name) {
        Ok(k) => k,
        Err(e) => { eprintln!("{}", e); std::process::exit(2); }
    };
    let mut examples: Vec<String> = fs::read_dir(examples_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma")).collect();
    examples.sort();
    let mut compile_fail = Vec::new();
    let mut crash = Vec::new();
    let mut pass = Vec::new();
    let mut timeout = Vec::new();
    let mut exec_fail = Vec::new();
    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = fs::read_to_string(&path).unwrap();
        let binary = match compile_for_backend(&source, kind) {
            Ok(b) => b,
            Err(e) => { compile_fail.push((ex.clone(), e)); continue; }
        };
        if let Some(q) = qemu {
            let (code, _stdout, stderr, crashed) = execute_binary(&binary, q, 2);
            if crashed {
                let err_str = String::from_utf8_lossy(&stderr);
                let err_short: String = err_str.chars().take(200).collect();
                crash.push((ex.clone(), code, err_short));
            } else if code == 124 {
                timeout.push((ex.clone(), code));
            } else if code != 0 && code != 42 {
                exec_fail.push((ex.clone(), code));
            } else {
                pass.push((ex.clone(), code));
            }
        } else {
            pass.push((ex.clone(), 0));
        }
    }
    println!("\n=== {} diagnostic results ===", backend_name);
    println!("Total: {} examples", examples.len());
    println!("Compile failures ({}):", compile_fail.len());
    for (n, e) in &compile_fail { println!("  X {} : {}", n, e); }
    println!("Crashes ({}):", crash.len());
    for (n, c, e) in &crash { println!("  CRASH {} (code={}): {}", n, c, e); }
    println!("Timeouts ({}):", timeout.len());
    for (n, c) in &timeout { println!("  TIMEOUT {} (code={})", n, c); }
    println!("Exec fail ({}):", exec_fail.len());
    for (n, c) in &exec_fail { println!("  FAIL {} (code={})", n, c); }
    println!("Pass ({}):", pass.len());
    for (n, c) in &pass { println!("  OK {} (code={})", n, c); }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "diag" {
        let backend = if args.len() > 2 { args[2].as_str() } else { "mips64" };
        let examples_dir = if args.len() > 3 { args[3].as_str() } else { "/tmp/vuma/examples" };
        let qemu: Option<&str> = if args.len() > 4 { Some(args[4].as_str()) } else { None };
        run_diag(backend, examples_dir, qemu);
        return;
    }
    let path = &args[1];
    let out_path = &args[2];
    let backend_name = if args.len() > 3 { args[3].as_str() } else { "aarch64" };
    let kind = backend_from_name(backend_name).unwrap_or(BackendKind::AArch64);
    let source = std::fs::read_to_string(path).unwrap();
    let binary = compile_for_backend(&source, kind).unwrap();
    std::fs::write(out_path, &binary).unwrap();
    eprintln!("Wrote {} bytes to {}", binary.len(), out_path);
}
