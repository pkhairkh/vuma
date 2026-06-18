use vuma::pipeline::{compile, CompileConfig, OptLevel, VerificationLevel};
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{run_scg_transforms, CompileTarget, bridge_scg_to_codegen};
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

fn execute_binary(binary: &[u8], qemu: Option<&str>, timeout_secs: u64) -> (i32, Vec<u8>, bool) {
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
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            let crashed = stderr.contains("Segmentation fault") || stderr.contains("uncaught target signal");
            (code, o.stdout, crashed)
        }
        Err(_) => (-1, vec![], true),
    }
}

#[test]
fn test_comprehensive_all_programs_all_backends() {
    let examples_dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut examples: Vec<String> = fs::read_dir(&examples_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma")).collect();
    examples.sort();

    // Part 1: Per-backend compilation (encode_program) — ALL 8 backends
    let backends = [
        ("x86_64", BackendKind::X86_64, None as Option<&str>),
        ("aarch64", BackendKind::AArch64, Some("/tmp/qemu_bins/qemu-aarch64") as Option<&str>),
        ("riscv64", BackendKind::RiscV64, Some("/tmp/qemu_bins/qemu-riscv64") as Option<&str>),
        ("arm32", BackendKind::Arm32, Some("/tmp/qemu_bins/qemu-arm") as Option<&str>),
        ("mips64", BackendKind::Mips64, Some("/tmp/qemu_bins/qemu-mips64") as Option<&str>),
        ("ppc64", BackendKind::PowerPC64, Some("/tmp/qemu_bins/qemu-ppc64") as Option<&str>),
        ("loongarch64", BackendKind::LoongArch64, Some("/tmp/qemu_bins/qemu-loongarch64") as Option<&str>),
        ("wasm32", BackendKind::Wasm32, None as Option<&str>),
    ];

    let expected_exits: std::collections::HashMap<&str, i32> = [
        ("minimal.vuma", 0), ("test_exit.vuma", 42),
        ("test_alloc.vuma", 0), ("test_call.vuma", 42),
    ].iter().cloned().collect();

    eprintln!("\n╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  COMPREHENSIVE TEST: {} programs × {} backends = {} combinations  ║", 
        examples.len(), backends.len(), examples.len() * backends.len());
    eprintln!("╚══════════════════════════════════════════════════════════════════╝\n");

    let mut compile_pass = 0; let mut compile_fail = 0;
    let mut exec_pass = 0; let mut exec_crash = 0; let mut exec_timeout = 0;
    let mut exec_skip = 0; let mut verified = 0; let mut wrong = 0;

    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = fs::read_to_string(&path).unwrap();
        let expected = expected_exits.get(ex.as_str()).copied();
        
        for (bname, kind, qemu) in &backends {
            let label = format!("{:<25} / {:<12}", ex, bname);
            
            match compile_per_backend(&source, *kind) {
                Err(e) => { compile_fail += 1; eprintln!("❌ COMPILE  {} {}", label, &e[..e.len().min(30)]); continue; }
                Ok(_) => { compile_pass += 1; }
            }
            
            if bname == &"wasm32" || (qemu.is_none() && bname != &"x86_64") {
                exec_skip += 1; continue;
            }
            
            let binary = compile_per_backend(&source, *kind).unwrap();
            let (code, stdout, crashed) = execute_binary(&binary, *qemu, 1);
            
            if crashed {
                exec_crash += 1;
                eprintln!("💥 CRASH    {} signal 11", label);
            } else if code == 124 {
                exec_timeout += 1;
                if !stdout.is_empty() { eprintln!("⏰ TIMEOUT  {} stdout={}B", label, stdout.len()); }
                else { eprintln!("⏰ TIMEOUT  {}", label); }
            } else {
                exec_pass += 1;
                match expected {
                    Some(exp) if code == exp => { verified += 1; eprintln!("✅ VERIFIED {} exit={} ✓", label, code); }
                    Some(exp) => { wrong += 1; eprintln!("⚠  WRONG    {} exit={} expected={}", label, code, exp); }
                    None => {
                        if !stdout.is_empty() { eprintln!("✅ EXEC     {} exit={} stdout={}B: {:?}", label, code, stdout.len(), &String::from_utf8_lossy(&stdout)[..stdout.len().min(50)]); }
                        else { eprintln!("✅ EXEC     {} exit={}", label, code); }
                    }
                }
            }
        }
    }

    // Part 2: Pipeline path (AArch64 with _start stub + syscall trampolines)
    eprintln!("\n--- Pipeline path (emit_elf: _start stub + syscall trampolines) ---");
    let config = CompileConfig { opt_level: OptLevel::O0, verification_level: VerificationLevel::None, ..Default::default() };
    let qemu_aarch64 = "/tmp/qemu_bins/qemu-aarch64";
    let mut pipe_pass = 0; let mut pipe_crash = 0; let mut pipe_timeout = 0;
    let mut pipe_verified = 0; let mut pipe_wrong = 0; let mut pipe_stdout = 0;
    
    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = fs::read_to_string(&path).unwrap();
        let expected = expected_exits.get(ex.as_str()).copied();
        let label = format!("{:<25} / pipeline", ex);
        
        match compile(&source, &config) {
            Err(_) => { eprintln!("❌ COMPILE  {} failed", label); continue; }
            Ok(output) => {
                let (code, stdout, crashed) = execute_binary(&output.binary, Some(qemu_aarch64), 2);
                if crashed { pipe_crash += 1; eprintln!("💥 CRASH    {} signal 11", label); }
                else if code == 124 { pipe_timeout += 1; 
                    if !stdout.is_empty() { pipe_stdout += 1; eprintln!("⏰ TIMEOUT  {} stdout={}B: {:?}", label, stdout.len(), &String::from_utf8_lossy(&stdout)[..stdout.len().min(50)]); }
                    else { eprintln!("⏰ TIMEOUT  {}", label); }
                }
                else {
                    pipe_pass += 1;
                    if !stdout.is_empty() { pipe_stdout += 1; }
                    match expected {
                        Some(exp) if code == exp => { pipe_verified += 1; eprintln!("✅ VERIFIED {} exit={} ✓ (expected {})", label, code, exp); }
                        Some(exp) => { pipe_wrong += 1; eprintln!("⚠  WRONG    {} exit={} (expected {})", label, code, exp); }
                        None => {
                            if !stdout.is_empty() { eprintln!("✅ EXEC     {} exit={} stdout={}B: {:?}", label, code, stdout.len(), &String::from_utf8_lossy(&stdout)[..stdout.len().min(50)]); }
                            else { eprintln!("✅ EXEC     {} exit={}", label, code); }
                        }
                    }
                }
            }
        }
    }

    eprintln!("\n╔══════════════════════════════════════════════════════════════════╗");
    eprintln!("║  SUMMARY                                                        ║");
    eprintln!("╠══════════════════════════════════════════════════════════════════╣");
    eprintln!("║  ENCODE_PROGRAM PATH (per-backend):                             ║");
    eprintln!("║    Compile:    {:>4} pass, {:>4} fail                              ║", compile_pass, compile_fail);
    eprintln!("║    Execute:    {:>4} pass, {:>4} crash, {:>4} timeout, {:>3} skip     ║", exec_pass, exec_crash, exec_timeout, exec_skip);
    eprintln!("║    Verified:   {:>4} correct exit, {:>4} wrong exit                 ║", verified, wrong);
    eprintln!("║  PIPELINE PATH (emit_elf, AArch64 via QEMU):                    ║");
    eprintln!("║    Execute:    {:>4} pass, {:>4} crash, {:>4} timeout                ║", pipe_pass, pipe_crash, pipe_timeout);
    eprintln!("║    Verified:   {:>4} correct exit, {:>4} wrong exit                 ║", pipe_verified, pipe_wrong);
    eprintln!("║    Has stdout: {:>4}                                              ║", pipe_stdout);
    eprintln!("╚══════════════════════════════════════════════════════════════════╝");
}
