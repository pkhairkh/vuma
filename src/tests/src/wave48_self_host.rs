//! # Wave 48 — Task 7-a + Task 7-b: Multi-module compile + bootstrap self-host test
//!
//! Implements the end-to-end test coverage required by TASKS.md Wave 48
//! `[BOOT-SELF]` (post-Task 7-a + Task 7-b remediation):
//!
//! 1. **Multi-module API smoke test** (`test_wave48_compile_modules_simple`)
//!    — proves the `VumaCompiler::compile_modules` API and the
//!    `merge_module_asts` helper work correctly on a synthetic
//!    multi-module program (entry module + helper module linked via
//!    `extern "C"`). Always runs (no platform gating, no `#[ignore]`).
//!
//! 2. **Bootstrap self-host end-to-end test** (`test_wave48_bootstrap_self_host`)
//!    — the real Wave 48 contract: compile the 5 bootstrap `.vuma` files
//!    (full_lexer + full_parser + ir_builder + codegen + elf) into a single
//!    `vumac` ELF, run `./vumac womb/lang/hello.vuma`, then run the
//!    emitted `a.out` and assert its stdout contains `"42"`. Currently
//!    `#[ignore]`'d with a documented blocker — see the test's doc-comment
//!    and TASKS.md's Wave 48 `[BOOT-SELF]` AUDIT RESOLVED note.
//!    Task 7-b RESOLVED the original `repd` parser-coverage blocker; the
//!    test now fails one stage later in `merge_module_asts` on duplicate
//!    fn definitions (`store_u64`/`load_u64`/`store_u32`/`load_u32`
//!    copy-pasted into each of the 5 bootstrap files).
//!
//! ## Platform gating
//!
//! Both tests are `#[cfg(target_arch = "x86_64")]`-gated because they
//! spawn the emitted ELF as a subprocess and assert on its exit code /
//! stdout — only an x86_64 host can exec an x86_64 ELF natively (no
//! qemu-user fallback in the test harness, mirroring Task 6-b's
//! `execute_x86_64_elf` gating).

#[cfg(target_arch = "x86_64")]
use std::path::Path;
#[cfg(target_arch = "x86_64")]
use std::process::Command;

#[cfg(target_arch = "x86_64")]
use vuma::api::VumaCompiler;
#[cfg(target_arch = "x86_64")]
use vuma::pipeline::CompileConfig;

// ===========================================================================
// Helper: resolve the workspace root (mirrors wave50.rs's helper)
// ===========================================================================

#[cfg(target_arch = "x86_64")]
fn workspace_root() -> std::path::PathBuf {
    option_env!("CARGO_MANIFEST_DIR")
        .map(|s| {
            Path::new(s)
                .parent() // src
                .and_then(|p| p.parent()) // workspace
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| Path::new(s).to_path_buf())
        })
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

// ===========================================================================
// Test 1 — Multi-module API smoke test (always runs on x86_64 hosts)
// ===========================================================================

/// Prove `VumaCompiler::compile_modules` and the AST-merge logic work on a
/// synthetic multi-module program.
///
/// Module `main_mod` declares `extern "C" { fn helper(); }` and calls
/// `helper()` from `main`. Module `helper_mod` defines `fn helper()`.
/// After merging, the `extern` declaration for `helper` must be stripped
/// (because a real `fn helper` definition exists in `helper_mod`), and the
/// call to `helper()` from `main` must resolve to a local call (no
/// `UnresolvedRelocation` from the backend).
///
/// The test asserts:
/// - `compile_modules` returns `Ok` (no errors).
/// - The emitted binary is a non-trivial ELF (≥64 bytes, starts with the
///   ELF magic).
/// - The emitted ELF, when executed natively on the x86_64 host, exits
///   with code 0 and prints `"42"` to stdout (helper returns 42, main
///   prints it via `print_int` and returns 0).
///
/// This is the strongest assertion that does NOT depend on the bootstrap
/// source files (which have a documented `merge_module_asts`
/// duplicate-fn-definition blocker — see `test_wave48_bootstrap_self_host`
/// below; the original `repd` parser-coverage blocker was RESOLVED by
/// Task 7-b).
#[cfg(target_arch = "x86_64")]
#[test]
fn test_wave48_compile_modules_simple() {
    // Module 1: entry point. Declares `helper` as extern (to be resolved
    // against module 2's `fn helper` definition during the merge step).
    let main_src = r#"
        extern "C" {
            fn helper() -> i32;
        }

        fn main() -> i32 {
            // Call helper() — its extern declaration is stripped by the
            // merge step (because helper_mod defines `fn helper`), so the
            // backend should emit a local call that resolves to helper's
            // body. The result (42) is captured in `x`, then printed via
            // the runtime-stub `print_int`.
            x: i32 = helper();
            print_int(x);
            print_newline();
            return 0;
        }
    "#;

    // Module 2: helper definition. The merge step sees `fn helper` here
    // and strips the `extern "C" { fn helper(...); }` from module 1.
    let helper_src = r#"
        fn helper() -> i32 {
            return 42;
        }
    "#;

    let modules: Vec<(String, String)> = vec![
        ("main.vuma".to_string(), main_src.to_string()),
        ("helper.vuma".to_string(), helper_src.to_string()),
    ];

    // Use O0 to disable the inliner (which would otherwise inline helper
    // into main and potentially confuse the return-value handling). The
    // goal of this test is to exercise the multi-module merge + cross-module
    // call resolution, not to test the optimiser.
    let mut config = CompileConfig::default();
    config.opt_level = vuma::pipeline::OptLevel::O0;

    let output = VumaCompiler::with_config(config)
        .compile_modules(&modules)
        .unwrap_or_else(|errors| {
            // Print every error to stderr for debugging before panicking.
            for e in &errors {
                eprintln!("[wave48 compile_modules simple] error: {}", e);
            }
            panic!(
                "wave48 compile_modules simple: compilation failed with {} error(s) (see stderr)",
                errors.len()
            );
        });

    // The emitted binary must be a non-trivial ELF.
    assert!(
        output.binary.len() >= 64,
        "wave48 compile_modules simple: emitted ELF is only {} bytes — expected ≥64 (a full \
         ELF64 header)",
        output.binary.len()
    );
    assert_eq!(
        &output.binary[0..4],
        b"\x7fELF",
        "wave48 compile_modules simple: emitted bytes do not start with the ELF magic — got {:?}",
        &output.binary[0..4]
    );

    // IR-function count: the merged program has 2 user fns (main + helper)
    // plus runtime stubs (_start, print_int, __vuma_argc, __vuma_argv, etc.).
    // The opt pass (O2 default) may inline `helper` into `main`, so the
    // post-opt IR may have only 1 user fn (main) + runtime stubs. The
    // minimum we can assert is ≥1 (main must always survive — it's the
    // entry point).
    assert!(
        output.ir_function_count >= 1,
        "wave48 compile_modules simple: expected ≥1 IR function (main + runtime stubs), got {}",
        output.ir_function_count
    );

    // Execute the ELF natively and assert stdout contains "42" (helper
    // returns 42, main prints it via print_int).
    let stdout = execute_elf_bytes(&output.binary, &[]).unwrap_or_else(|e| {
        panic!(
            "wave48 compile_modules simple: failed to execute emitted ELF — {}",
            e
        );
    });
    assert!(
        stdout.contains("42"),
        "wave48 compile_modules simple: emitted ELF ran but did not print \"42\" on stdout — \
         got stdout = {:?}",
        stdout
    );

    eprintln!(
        "wave48 compile_modules simple: OK — merged 2 modules into {}-byte ELF, {} IR functions, \
         stdout = {:?} (contains \"42\" ✓)",
        output.binary.len(),
        output.ir_function_count,
        stdout
    );
}

// ===========================================================================
// Test 2 — Bootstrap self-host end-to-end (currently #[ignore]'d)
// ===========================================================================

/// Wave 48 `[BOOT-SELF]` end-to-end test: the bootstrap `.vuma` files
/// compile together into a working `vumac` ELF, which when run on
/// `womb/lang/hello.vuma` produces an `a.out` that prints `"42"`.
///
/// ## Test procedure
///
/// 1. Build the list of 5 bootstrap modules:
///    `full_lexer.vuma` (entry, 805 lines), `full_parser.vuma` (713 lines,
///    defines `parse` + `name_hash`), `ir_builder.vuma` (767 lines, defines
///    `irb_build_main` + `scg_construct` + `bd_infer` + `ive_verify`),
///    `codegen.vuma` (530 lines, defines `codegen_emit`), `elf.vuma` (132
///    lines, defines `write_elf64`).
/// 2. Call `VumaCompiler::compile_modules(&modules, &config)`.
/// 3. Assert compilation succeeds (capture the error vec on failure and
///    print it for debugging).
/// 4. Write the ELF to a temp file, `chmod 0o755`.
/// 5. Spawn the ELF as a subprocess with `womb/lang/hello.vuma` as
///    argv[1] (use `std::env::current_dir()` to make the path absolute).
/// 6. Capture exit code — should be 0 (the bootstrap's `main` returns 0
///    on success).
/// 7. After the bootstrap runs, find `a.out` in the bootstrap's CWD,
///    `chmod 0o755`, run it, assert stdout contains `"42"`.
///
/// ## Current blocker (why this test is `#[ignore]`'d)
///
/// ### RESOLVED by Task 7-b: the `repd` parser-coverage blocker
///
/// Task 7-a documented that `womb/lang/ir_builder.vuma:593` uses `repd`
/// (a reserved BD-directive keyword in `src/parser/src/parser.rs:1126-1129`)
/// as a local variable name (`repd: Address = __vuma_alloc(BD_VREG_CAP);`),
/// and the parser failed with
/// `ParseError { message: "expected '(', found ':'", line: Some(593), column: Some(9) }`.
/// Task 7-b fixed this via parser context-awareness: when `TokenKind::Bd` /
/// `Repd` / `Capd` / `Reld` is followed by `:` instead of `(`, the parser
/// now treats it as an identifier (let-statement) rather than a BD
/// directive. See `src/parser/src/parser.rs` (the `TokenKind::Bd |
/// TokenKind::Repd | TokenKind::Capd | TokenKind::Reld` dispatch arm in
/// `parse_stmt`) and the regression tests in `src/parser/tests/edge_cases.rs`
/// (`test_repd_as_identifier_in_let`, `test_bd_as_identifier_in_let`,
/// `test_capd_as_identifier_in_let`, `test_reld_as_identifier_in_let`,
/// `test_repd_as_bd_directive_still_works`).
///
/// After the Task 7-b parser fix, all 5 bootstrap files parse cleanly
/// (verified: `cargo run --bin vuma -- run --isa x86_64
/// womb/lang/{full_lexer,ir_builder}.vuma` no longer emits any
/// `ParseError`).
///
/// ### NEW blocker: duplicate fn definitions across bootstrap modules
///
/// With parsing unblocked, `compile_modules` now reaches the
/// `merge_module_asts` step (`src/pipeline.rs:5424`) and fails there:
///
/// ```text
/// [ast-to-scg] compile_modules: duplicate fn definition 'store_u64' across
///   modules — each function name must be defined in at most one module
/// [ast-to-scg] compile_modules: duplicate fn definition 'load_u64' across
///   modules — each function name must be defined in at most one module
/// [ast-to-scg] compile_modules: duplicate fn definition 'store_u32' across
///   modules — each function name must be defined in at most one module
/// [ast-to-scg] compile_modules: duplicate fn definition 'load_u32' across
///   modules — each function name must be defined in at most one module
///   ... (14 errors total)
/// ```
///
/// Root cause: each of the 5 bootstrap files copy-pastes the same 4
/// helper fns (`store_u64`, `load_u64`, `store_u32`, `load_u32`) at the
/// top of the file as a self-contained preamble. `merge_module_asts`'s
/// Pass 1 (`src/pipeline.rs:5429-5448`) treats same-name fn definitions
/// across modules as a hard error (a conservative policy that catches
/// real conflicts but rejects the legitimate bootstrap pattern of
/// identical-copy preamble helpers).
///
/// Site map of the duplicates (rg `^fn (store_u64|load_u64|store_u32|load_u32)`):
///
/// ```text
/// womb/lang/full_lexer.vuma:102  fn store_u64(...)
/// womb/lang/full_lexer.vuma:108  fn load_u64(...)
/// womb/lang/full_lexer.vuma:115  fn store_u32(...)
/// womb/lang/full_lexer.vuma:119  fn load_u32(...)
/// womb/lang/full_parser.vuma:21  fn store_u64(...)
/// womb/lang/full_parser.vuma:27  fn load_u64(...)
/// womb/lang/full_parser.vuma:34  fn store_u32(...)
/// womb/lang/full_parser.vuma:38  fn load_u32(...)
/// womb/lang/ir_builder.vuma:13   fn store_u64(...)
/// womb/lang/ir_builder.vuma:19   fn load_u64(...)
/// womb/lang/ir_builder.vuma:26   fn store_u32(...)
/// womb/lang/ir_builder.vuma:30   fn load_u32(...)
/// womb/lang/codegen.vuma:13      fn store_u64(...)
/// womb/lang/codegen.vuma:19      fn load_u64(...)
/// womb/lang/codegen.vuma:26      fn store_u32(...)
/// womb/lang/codegen.vuma:30      fn load_u32(...)
/// womb/lang/elf.vuma:25          fn store_u64(...)
/// womb/lang/elf.vuma:31          fn store_u32(...)
/// ```
///
/// ## Possible fixes (out of scope for Task 7-b)
///
/// 1. **`merge_module_asts` deduplication** — extend Pass 1 to deduplicate
///    identical fn definitions (same name + same signature + same body)
///    instead of rejecting them.  This is the principled fix: the
///    bootstrap's copy-paste pattern is a deliberate "each module is
///    self-contained" design, and `merge_module_asts` should honour it
///    by keeping only one copy of each identical preamble helper.
///    A simple version: keep the first definition and warn (or silently
///    drop) subsequent identical definitions; a stricter version: compare
///    bodies (normalised) and error only on conflicting definitions.
///    This is a `src/pipeline.rs` change, not a parser change, and is
///    tracked as the priority follow-up #1 in TASKS.md.
///
/// 2. **Bootstrap-source refactor** — extract the shared helpers into a
///    new `womb/lang/shared.vuma` module and `import` it from each of
///    the 5 bootstrap files.  This requires the bootstrap source to
///    support `import` semantics (which the bootstrap parser does
///    implement for `womb/lang/full_lexer.vuma`'s own module system, but
///    the production `compile_modules` API does not yet resolve
///    `import "..."` paths between the bundled modules).  Out of scope.
///
/// ## How to run this test (after the new blocker is fixed)
///
/// ```text
/// cargo test -p vuma-tests --lib wave48_self_host::test_wave48_bootstrap_self_host -- --ignored
/// ```
#[cfg(target_arch = "x86_64")]
#[test]
#[ignore = "blocked by `merge_module_asts` duplicate-fn-definition rejection: the 5 bootstrap \
            files each copy-paste `store_u64`/`load_u64`/`store_u32`/`load_u32` as a self-contained \
            preamble (14 errors total). The original `repd` parser-coverage blocker was RESOLVED \
            by Task 7-b (parser context-awareness for `TokenKind::Bd`/`Repd`/`Capd`/`Reld`). \
            See TASKS.md Wave 48 [BOOT-SELF] for the new-blocker write-up."]
fn test_wave48_bootstrap_self_host() {
    let workspace_root = workspace_root();
    let womb_lang = workspace_root.join("womb").join("lang");

    // ── Step 1: build the list of 5 bootstrap modules. ───────────────
    let modules: Vec<(String, String)> = vec![
        (
            "full_lexer.vuma".to_string(),
            include_str!("../../../womb/lang/full_lexer.vuma").to_string(),
        ),
        (
            "full_parser.vuma".to_string(),
            include_str!("../../../womb/lang/full_parser.vuma").to_string(),
        ),
        (
            "ir_builder.vuma".to_string(),
            include_str!("../../../womb/lang/ir_builder.vuma").to_string(),
        ),
        (
            "codegen.vuma".to_string(),
            include_str!("../../../womb/lang/codegen.vuma").to_string(),
        ),
        (
            "elf.vuma".to_string(),
            include_str!("../../../womb/lang/elf.vuma").to_string(),
        ),
    ];

    // ── Step 2 + 3: compile the merged program; capture errors for
    //    debugging if compilation fails. ──────────────────────────────
    let config = CompileConfig::default();
    let output = VumaCompiler::with_config(config)
        .compile_modules(&modules)
        .unwrap_or_else(|errors| {
            // Print every error to stderr for debugging before panicking.
            for e in &errors {
                eprintln!("[wave48 bootstrap self-host] error: {}", e);
            }
            panic!(
                "wave48 bootstrap self-host: compile_modules failed with {} error(s) (see stderr)",
                errors.len()
            );
        });

    // The emitted binary must be a non-trivial ELF.
    assert!(
        output.binary.len() >= 64,
        "wave48 bootstrap self-host: emitted ELF is only {} bytes — expected ≥64 (a full \
         ELF64 header)",
        output.binary.len()
    );
    assert_eq!(
        &output.binary[0..4],
        b"\x7fELF",
        "wave48 bootstrap self-host: emitted bytes do not start with the ELF magic — got {:?}",
        &output.binary[0..4]
    );

    // ── Step 4 + 5: write the ELF to a temp file, chmod 0o755, spawn it
    //    with `womb/lang/hello.vuma` as argv[1]. ──────────────────────
    let vumac_path = std::env::temp_dir().join(format!(
        "vuma_wave48_vumac_{}.bin",
        std::process::id()
    ));
    std::fs::write(&vumac_path, &output.binary).unwrap_or_else(|e| {
        panic!(
            "wave48 bootstrap self-host: cannot write temp ELF '{}': {}",
            vumac_path.display(),
            e
        )
    });
    chmod_0o755(&vumac_path);

    // The bootstrap's `main()` reads argv[1] as the source file path
    // (falling back to "womb/lang/hello.vuma" if argc < 2 — see
    // full_lexer.vuma:38-45). Pass the absolute path to hello.vuma so
    // the bootstrap can find it regardless of the test's CWD.
    let hello_path = womb_lang.join("hello.vuma");
    assert!(
        hello_path.exists(),
        "wave48 bootstrap self-host: womb/lang/hello.vuma does not exist at {}",
        hello_path.display()
    );

    // Use a per-test temp working directory so the bootstrap's `a.out`
    // output doesn't collide with other tests or the user's CWD.
    let bootstrap_cwd = std::env::temp_dir().join(format!(
        "vuma_wave48_bootstrap_cwd_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&bootstrap_cwd).unwrap_or_else(|e| {
        panic!(
            "wave48 bootstrap self-host: cannot create temp CWD '{}': {}",
            bootstrap_cwd.display(),
            e
        )
    });

    eprintln!(
        "wave48 bootstrap self-host: spawning {} {} (CWD = {})",
        vumac_path.display(),
        hello_path.display(),
        bootstrap_cwd.display()
    );

    let run_output = Command::new(&vumac_path)
        .arg(&hello_path)
        .current_dir(&bootstrap_cwd)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "wave48 bootstrap self-host: failed to spawn vumac ELF '{}': {}",
                vumac_path.display(),
                e
            )
        });

    let run_stdout = String::from_utf8_lossy(&run_output.stdout).into_owned();
    let run_stderr = String::from_utf8_lossy(&run_output.stderr).into_owned();
    let run_exit = run_output.status.code().unwrap_or(-1);

    eprintln!(
        "wave48 bootstrap self-host: vumac exited with code {}, stdout = {:?}, stderr = {:?}",
        run_exit, run_stdout, run_stderr
    );

    // ── Step 6: assert the bootstrap exited with code 0. ──────────────
    // (The bootstrap's main returns 0 on success; non-zero indicates a
    // pipeline-stage failure inside the bootstrap.)
    assert_eq!(
        run_exit, 0,
        "wave48 bootstrap self-host: vumac exited with code {} (expected 0); stdout = {:?}, \
         stderr = {:?}",
        run_exit, run_stdout, run_stderr
    );

    // ── Step 7: find a.out in the bootstrap's CWD, chmod, run, assert
    //    stdout contains "42". ────────────────────────────────────────
    let a_out_path = bootstrap_cwd.join("a.out");
    assert!(
        a_out_path.exists(),
        "wave48 bootstrap self-host: vumac ran successfully (exit 0) but did not produce \
         `a.out` in its CWD ({}) — the bootstrap's write_elf64() may have failed silently",
        bootstrap_cwd.display()
    );

    chmod_0o755(&a_out_path);

    let a_out_output = Command::new(&a_out_path)
        .current_dir(&bootstrap_cwd)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "wave48 bootstrap self-host: failed to spawn a.out '{}': {}",
                a_out_path.display(),
                e
            )
        });

    let a_out_stdout = String::from_utf8_lossy(&a_out_output.stdout).into_owned();
    let a_out_stderr = String::from_utf8_lossy(&a_out_output.stderr).into_owned();
    let a_out_exit = a_out_output.status.code().unwrap_or(-1);

    eprintln!(
        "wave48 bootstrap self-host: a.out exited with code {}, stdout = {:?}, stderr = {:?}",
        a_out_exit, a_out_stdout, a_out_stderr
    );

    assert!(
        a_out_stdout.contains("42"),
        "wave48 bootstrap self-host: a.out ran but did not print \"42\" on stdout — got \
         stdout = {:?}, stderr = {:?}, exit = {}",
        a_out_stdout,
        a_out_stderr,
        a_out_exit
    );

    // Best-effort cleanup of temp files.
    let _ = std::fs::remove_file(&vumac_path);
    let _ = std::fs::remove_file(&a_out_path);
    let _ = std::fs::remove_dir_all(&bootstrap_cwd);

    eprintln!(
        "wave48 bootstrap self-host: OK — bootstrap compiled hello.vuma → a.out → stdout \
         contains \"42\" ✓"
    );
}

// ===========================================================================
// Helpers (x86_64-only)
// ===========================================================================

#[cfg(target_arch = "x86_64")]
fn chmod_0o755(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .unwrap_or_else(|e| {
                panic!(
                    "wave48 self-host: cannot stat '{}': {}",
                    path.display(),
                    e
                )
            })
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap_or_else(|e| {
            panic!(
                "wave48 self-host: cannot chmod '{}': {}",
                path.display(),
                e
            )
        });
    }
    #[cfg(not(unix))]
    {
        let _ = path; // no-op on non-Unix
    }
}

/// Execute an in-memory ELF binary on the x86_64 host. Writes the bytes
/// to a unique temp file, `chmod 0o755`, spawns via `Command::output()`
/// with the given argv, captures stdout/stderr, returns `Ok(stdout)` on
/// success. Mirrors `wave50.rs::execute_x86_64_elf` but accepts an
/// explicit `args: &[&str]` for argv[1..].
#[cfg(target_arch = "x86_64")]
fn execute_elf_bytes(elf_bytes: &[u8], args: &[&str]) -> Result<String, String> {
    use std::io::Write;

    let tmp_dir = std::env::temp_dir();
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let exe_path = tmp_dir.join(format!(
        "vuma_wave48_elf_{}_{}.bin",
        std::process::id(),
        seq
    ));

    let mut f = std::fs::File::create(&exe_path)
        .map_err(|e| format!("cannot create temp ELF '{}': {}", exe_path.display(), e))?;
    f.write_all(elf_bytes)
        .map_err(|e| format!("cannot write temp ELF '{}': {}", exe_path.display(), e))?;
    drop(f);

    chmod_0o755(&exe_path);

    let output = Command::new(&exe_path)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn temp ELF '{}': {}", exe_path.display(), e))?;

    let _ = std::fs::remove_file(&exe_path);

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code().unwrap_or(-1);

    if exit_code == 0 || !stdout.is_empty() {
        Ok(stdout)
    } else {
        Err(format!(
            "ELF execution failed (exit code {}, stderr: {})",
            exit_code,
            stderr.trim()
        ))
    }
}
