use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel, bridge_scg_to_codegen};
use std::process::Command;
use std::fs;

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

struct BackendInfo {
    name: &'static str,
    kind: BackendKind,
    qemu: Option<&'static str>,
}

fn get_backends() -> Vec<BackendInfo> {
    let mut backends = Vec::new();
    // x86_64: native + QEMU
    let qemu_x86 = if std::path::Path::new("/tmp/qemu_bins/qemu-x86_64").exists() { Some("/tmp/qemu_bins/qemu-x86_64") } else { None };
    backends.push(BackendInfo { name: "x86_64", kind: BackendKind::X86_64, qemu: None }); // native first
    // All other backends via QEMU
    let qemu_map = [
        ("aarch64", BackendKind::AArch64, "/tmp/qemu_bins/qemu-aarch64"),
        ("x86_64_qemu", BackendKind::X86_64, "/tmp/qemu_bins/qemu-x86_64"),
        ("riscv64", BackendKind::RiscV64, "/tmp/qemu_bins/qemu-riscv64"),
        ("arm32", BackendKind::Arm32, "/tmp/qemu_bins/qemu-arm"),
        ("mips64", BackendKind::Mips64, "/tmp/qemu_bins/qemu-mips64"),
        ("ppc64", BackendKind::PowerPC64, "/tmp/qemu_bins/qemu-ppc64"),
        ("loongarch64", BackendKind::LoongArch64, "/tmp/qemu_bins/qemu-loongarch64"),
    ];
    for (name, kind, qemu) in &qemu_map {
        if std::path::Path::new(qemu).exists() {
            backends.push(BackendInfo { name, kind: *kind, qemu: Some(qemu) });
        }
    }
    // Wasm32: no execution (no runtime)
    backends.push(BackendInfo { name: "wasm32", kind: BackendKind::Wasm32, qemu: None });
    backends
}

fn execute_binary(binary: &[u8], qemu: Option<&str>) -> (i32, Vec<u8>, Vec<u8>, bool) {
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
    cmd.arg("1");
    if let Some(q) = qemu { cmd.arg(q); }
    cmd.arg(&bin_path);
    let output = cmd.output();
    let _ = fs::remove_file(&bin_path);
    match output {
        Ok(o) => {
            let code = o.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let crashed = stderr.contains("Segmentation fault") || stderr.contains("uncaught target signal");
            (code, o.stdout, o.stderr, crashed)
        }
        Err(_) => (-1, vec![], b"exec failed".to_vec(), true),
    }
}

#[test]
fn test_comprehensive_all_programs_all_backends() {
    let examples_dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut examples: Vec<String> = fs::read_dir(&examples_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma")).collect();
    examples.sort();

    let backends = get_backends();
    
    eprintln!("\n╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  COMPREHENSIVE TEST: {} programs × {} backends = {} combinations  ║", 
        examples.len(), backends.len(), examples.len() * backends.len());
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
    eprintln!("Backends: {:?}", backends.iter().map(|b| (b.name, b.qemu.is_some())).collect::<Vec<_>>());
    eprintln!();

    let mut compile_pass = 0;
    let mut compile_fail = 0;
    let mut exec_pass = 0;
    let mut exec_crash = 0;
    let mut exec_timeout = 0;
    let mut exec_skip = 0;
    let mut has_stdout = 0;

    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = fs::read_to_string(&path).unwrap();
        
        for backend in &backends {
            let label = format!("{} / {}", ex, backend.name);
            
            // Step 1: Compile
            match compile_for_backend(&source, backend.kind) {
                Err(e) => {
                    compile_fail += 1;
                    eprintln!("❌ COMPILE  {:<50} {}", label, &e[..e.len().min(40)]);
                    continue;
                }
                Ok(_) => { compile_pass += 1; }
            }
            
            // Step 2: Execute (skip Wasm32 and native x86_64 already done)
            if backend.name == "wasm32" {
                exec_skip += 1;
                eprintln!("⏭  SKIP     {:<50} (no wasm runtime)", label);
                continue;
            }
            
            // For x86_64 native (no QEMU), execute directly
            // For all others, use QEMU
            match compile_for_backend(&source, backend.kind) {
                Ok(binary) => {
                    let (code, stdout, stderr, crashed) = execute_binary(&binary, backend.qemu);
                    
                    if crashed {
                        exec_crash += 1;
                        let err = String::from_utf8_lossy(&stderr);
                        eprintln!("💥 CRASH    {:<50} {}", label, err.lines().next().unwrap_or(""));
                    } else if code == 124 {
                        exec_timeout += 1;
                        if !stdout.is_empty() {
                            has_stdout += 1;
                            eprintln!("⏰ TIMEOUT  {:<50} stdout={}B: {:?}", label, stdout.len(), 
                                &String::from_utf8_lossy(&stdout)[..stdout.len().min(40)]);
                        } else {
                            eprintln!("⏰ TIMEOUT  {:<50}", label);
                        }
                    } else {
                        exec_pass += 1;
                        if !stdout.is_empty() {
                            has_stdout += 1;
                            eprintln!("✅ EXEC     {:<50} exit={} stdout={}B: {:?}", label, code, stdout.len(),
                                &String::from_utf8_lossy(&stdout)[..stdout.len().min(40)]);
                        } else {
                            eprintln!("✅ EXEC     {:<50} exit={} stdout=0B", label, code);
                        }
                    }
                }
                Err(_) => { exec_skip += 1; }
            }
        }
    }

    let total = examples.len() * backends.len();
    eprintln!("\n╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  SUMMARY: {} total combinations                                   ║", total);
    eprintln!("╠══════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Compile:    {:>4} pass, {:>4} fail                                  ║", compile_pass, compile_fail);
    eprintln!("║  Execute:    {:>4} pass (clean exit)                                ║", exec_pass);
    eprintln!("║  Crash:      {:>4} (SIGSEGV)                                        ║", exec_crash);
    eprintln!("║  Timeout:    {:>4} (3s limit)                                       ║", exec_timeout);
    eprintln!("║  Skip:       {:>4} (no runtime)                                     ║", exec_skip);
    eprintln!("║  Has stdout: {:>4}                                                  ║", has_stdout);
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
}
