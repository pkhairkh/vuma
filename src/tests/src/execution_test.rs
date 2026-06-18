use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{Parser, AstToScg};
use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel, bridge_scg_to_codegen};
use std::process::Command;

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

fn find_qemu(arch: &str) -> Option<String> {
    // Check PATH
    let qemu_name = format!("qemu-{}", arch);
    if let Ok(o) = Command::new(&qemu_name).arg("--version").output() {
        if o.status.success() { return Some(qemu_name); }
    }
    // Check /tmp
    let path = format!("/tmp/qemu_all/usr/bin/qemu-{}", arch);
    if std::path::Path::new(&path).exists() { return Some(path); }
    None
}

fn execute_binary(binary: &[u8], qemu: Option<&str>) -> Result<(i32, Vec<u8>, Vec<u8>), String> {
    let bin_path = std::env::temp_dir().join(format!("vuma_exec_{}.bin", std::process::id()));
    std::fs::write(&bin_path, binary).map_err(|e| format!("write: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin_path, perms).unwrap();
    }
    
    let mut cmd = match qemu {
        Some(q) => { let mut c = Command::new("timeout"); c.arg("3").arg(q).arg(&bin_path); c }
        None => { let mut c = Command::new("timeout"); c.arg("3").arg(&bin_path); c }
    };
    let output = cmd.output().map_err(|e| format!("exec: {}", e))?;
    
    let _ = std::fs::remove_file(&bin_path);
    let exit_code = output.status.code().unwrap_or(-1);
    Ok((exit_code, output.stdout, output.stderr))
}

#[test]
fn test_execute_all_examples_all_executable_backends() {
    let examples_dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut examples: Vec<String> = std::fs::read_dir(&examples_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".vuma")).collect();
    examples.sort();

    // Backends we can execute:
    // - x86_64: native (host is x86_64)
    // - AArch64: via QEMU (if available)
    // Other backends: compile-only (no QEMU available)
    let mut exec_backends: Vec<(&str, BackendKind, Option<String>)> = Vec::new();
    // x86_64: native execution (host is x86_64)
    exec_backends.push(("x86_64", BackendKind::X86_64, None));
    // AArch64: via QEMU if available
    if let Some(q) = find_qemu("aarch64") {
        exec_backends.push(("aarch64", BackendKind::AArch64, Some(q)));
    }

    eprintln!("\n=== Execution Test: {} examples × {} backends ===", examples.len(), exec_backends.len());
    eprintln!("Executable backends: {:?}", exec_backends.iter().map(|(n, _, q)| (n, q.is_some())).collect::<Vec<_>>());

    let mut pass = 0;
    let mut fail = 0;
    let mut crash = 0;

    for ex in &examples {
        let path = format!("{}/{}", examples_dir, ex);
        let source = std::fs::read_to_string(&path).unwrap();
        
        for (name, kind, qemu) in &exec_backends {
            match compile_for_backend(&source, *kind) {
                Err(e) => {
                    eprintln!("  ❌ COMPILE {} {}: {}", ex, name, e);
                    fail += 1;
                }
                Ok(binary) => {
                    match execute_binary(&binary, qemu.as_deref()) {
                        Ok((code, stdout, stderr)) => {
                            let stdout_str = String::from_utf8_lossy(&stdout);
                            let stderr_str = String::from_utf8_lossy(&stderr);
                            if stderr_str.contains("Segmentation fault") || code == -11 {
                                eprintln!("  💥 CRASH  {} {}: signal 11 (SIGSEGV)", ex, name);
                                crash += 1;
                            } else if stderr_str.contains("uncaught target signal") {
                                eprintln!("  💥 CRASH  {} {}: {}", ex, name, stderr_str.lines().next().unwrap_or(""));
                                crash += 1;
                            } else {
                                eprintln!("  ✅ EXEC   {} {}: exit={} stdout={}B", ex, name, code, stdout.len());
                                pass += 1;
                            }
                        }
                        Err(e) => {
                            eprintln!("  ❌ EXEC   {} {}: {}", ex, name, e);
                            fail += 1;
                        }
                    }
                }
            }
        }
    }

    eprintln!("\n=== Results: {} pass, {} fail, {} crash ===", pass, fail, crash);
}
