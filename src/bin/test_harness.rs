//! `test_harness` — master test orchestrator for the VUMA compiler.
//!
//! Wraps the library in [`vuma::test_harness`] with a small CLI that
//! supports four modes:
//!
//! - `single <file> <backend> [opt]` — one file, one backend.
//! - `all <backend> [opt] [examples_dir]` — every `.vuma` file in a
//!   directory, one backend.
//! - `differential [opt] [examples_dir]` — every file on every QEMU
//!   backend, reports programs where backends disagree.
//! - `opt_compare <file>` — one file at `O0` and `O3`, reports
//!   (backend, opt) pairs that disagree.
//!
//! Add `--json` anywhere in the args to switch from the human-readable
//! table to a JSON document suitable for CI ingestion.
//!
//! Exit code: `0` if every trial passed, `1` if any failed (crash,
//! timeout, compile error, or — in differential/opt_compare — any
//! disagreement), `2` for CLI usage errors.

use std::process::ExitCode;

use serde::Serialize;
use vuma::pipeline::OptLevel;
use vuma::test_harness::{
    all_qemu_backends, backend_from_name, backend_name, list_examples, opt_from_name, opt_name,
    run_test, run_test_with_timeout, TestResult,
};
use vuma_codegen::backend::BackendKind;

#[derive(Debug, Serialize)]
struct JsonOutput {
    mode: String,
    opt: Option<String>,
    results: Vec<JsonRow>,
    disagreements: Vec<String>,
    summary: JsonSummary,
}

#[derive(Debug, Serialize)]
struct JsonRow {
    file: String,
    backend: String,
    opt: String,
    result: TestResult,
}

#[derive(Debug, Serialize)]
struct JsonSummary {
    total: usize,
    passed: usize,
    failed: usize,
}

fn main() -> ExitCode {
    let raw_args: Vec<String> = std::env::args().collect();
    let json_mode = raw_args.iter().any(|a| a == "--json");
    let args: Vec<String> = raw_args
        .iter()
        .filter(|a| *a != "--json")
        .cloned()
        .collect();

    if args.len() < 2 {
        eprintln!("usage: test_harness <mode> [options] [--json]");
        eprintln!("modes:");
        eprintln!("  single <file> <backend> [opt_level]");
        eprintln!("  all <backend> [opt_level] [examples_dir]");
        eprintln!("  differential [opt_level] [examples_dir]");
        eprintln!("  opt_compare <file>");
        return ExitCode::from(2);
    }

    let mode = args[1].as_str();
    let rest: Vec<String> = args[2..].to_vec();

    match mode {
        "single" => run_single(&rest, json_mode),
        "all" => run_all(&rest, json_mode),
        "differential" => run_differential(&rest, json_mode),
        "opt_compare" => run_opt_compare(&rest, json_mode),
        "help" | "--help" | "-h" => {
            eprintln!("usage: test_harness <mode> [options] [--json]");
            eprintln!("modes:");
            eprintln!("  single <file> <backend> [opt_level]");
            eprintln!("  all <backend> [opt_level] [examples_dir]");
            eprintln!("  differential [opt_level] [examples_dir]");
            eprintln!("  opt_compare <file>");
            ExitCode::from(0)
        }
        other => {
            eprintln!("unknown mode: {}", other);
            ExitCode::from(2)
        }
    }
}

// A flat row used by both the table renderer and the JSON builder.
#[derive(Debug, Clone)]
struct Row {
    file: String,
    backend: BackendKind,
    opt: OptLevel,
    result: TestResult,
}

// ── single ─────────────────────────────────────────────────────────────

fn run_single(args: &[String], json: bool) -> ExitCode {
    if args.is_empty() {
        eprintln!("usage: test_harness single <file> <backend> [opt_level]");
        return ExitCode::from(2);
    }
    let file = &args[0];
    let backend_name_s = args.get(1).map(|s| s.as_str()).unwrap_or("x86_64");
    let opt_s = args.get(2).map(|s| s.as_str()).unwrap_or("O0");

    let backend = match backend_from_name(backend_name_s) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };
    let opt = match opt_from_name(opt_s) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    let results = run_test(file, &[backend], opt);
    let rows: Vec<Row> = results
        .iter()
        .map(|(b, r)| Row {
            file: file.clone(),
            backend: *b,
            opt,
            result: r.clone(),
        })
        .collect();
    let all_pass = rows.iter().all(|r| r.result.is_pass());

    if json {
        let out = build_json("single", Some(opt_name(opt)), rows, Vec::new());
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print_table(&rows);
        print_summary_line(rows.len(), rows.iter().filter(|r| r.result.is_pass()).count());
    }

    exit_code(all_pass, &[])
}

// ── all ────────────────────────────────────────────────────────────────

fn run_all(args: &[String], json: bool) -> ExitCode {
    let backend_name_s = args.get(0).map(|s| s.as_str()).unwrap_or("x86_64");
    let opt_s = args.get(1).map(|s| s.as_str()).unwrap_or("O0");
    let examples_dir = args.get(2).map(|s| s.as_str()).unwrap_or("examples");

    let backend = match backend_from_name(backend_name_s) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };
    let opt = match opt_from_name(opt_s) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    let programs = list_examples(examples_dir);
    if programs.is_empty() {
        eprintln!("no .vuma files found in {}", examples_dir);
        return ExitCode::from(2);
    }

    let mut rows: Vec<Row> = Vec::new();
    for prog in &programs {
        let rs = run_test(prog, &[backend], opt);
        for (b, r) in rs {
            rows.push(Row {
                file: prog.clone(),
                backend: b,
                opt,
                result: r,
            });
        }
    }

    let total = rows.len();
    let passed = rows.iter().filter(|r| r.result.is_pass()).count();

    if json {
        let out = build_json("all", Some(opt_name(opt)), rows.clone(), Vec::new());
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print_table(&rows);
        print_summary_line(total, passed);
    }

    exit_code(passed == total, &[])
}

// ── differential ───────────────────────────────────────────────────────

fn run_differential(args: &[String], json: bool) -> ExitCode {
    let opt_s = args.get(0).map(|s| s.as_str()).unwrap_or("O0");
    let examples_dir = args.get(1).map(|s| s.as_str()).unwrap_or("examples");

    let opt = match opt_from_name(opt_s) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    let backends = all_qemu_backends();
    let programs = list_examples(examples_dir);
    if programs.is_empty() {
        eprintln!("no .vuma files found in {}", examples_dir);
        return ExitCode::from(2);
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut disagreements: Vec<String> = Vec::new();

    for prog in &programs {
        let rs = run_test_with_timeout(prog, &backends, opt, 3);
        // Compare exit codes across backends that Pass.
        let codes: Vec<Option<i32>> = rs
            .iter()
            .map(|(_, r)| match &r {
                TestResult::Pass { exit_code, .. } => Some(*exit_code),
                _ => None,
            })
            .collect();
        let all_pass = codes.iter().all(|c| c.is_some());
        let all_agree =
            all_pass && codes.iter().all(|c| *c == codes.first().copied().flatten());
        if !all_agree {
            disagreements.push(prog.clone());
        }
        for (b, r) in rs {
            rows.push(Row {
                file: prog.clone(),
                backend: b,
                opt,
                result: r,
            });
        }
    }

    let total = rows.len();
    let passed = rows.iter().filter(|r| r.result.is_pass()).count();

    if json {
        let out = build_json(
            "differential",
            Some(opt_name(opt)),
            rows.clone(),
            disagreements.clone(),
        );
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print_table(&rows);
        println!("\n=== Differential summary ===");
        println!(
            "  Programs with cross-backend disagreement: {}",
            disagreements.len()
        );
        for d in &disagreements {
            println!("    DIFF  {}", d);
        }
        print_summary_line(total, passed);
    }

    exit_code(passed == total, &disagreements)
}

// ── opt_compare ────────────────────────────────────────────────────────

fn run_opt_compare(args: &[String], json: bool) -> ExitCode {
    if args.is_empty() {
        eprintln!("usage: test_harness opt_compare <file>");
        return ExitCode::from(2);
    }
    let file = &args[0];
    let backends = all_qemu_backends();

    let mut rows: Vec<Row> = Vec::new();
    let mut per_backend_per_opt: std::collections::HashMap<
        BackendKind,
        Vec<(OptLevel, Option<i32>)>,
    > = std::collections::HashMap::new();

    for opt in [OptLevel::O0, OptLevel::O3] {
        let rs = run_test_with_timeout(file, &backends, opt, 3);
        for (b, r) in rs {
            let code = match &r {
                TestResult::Pass { exit_code, .. } => Some(*exit_code),
                _ => None,
            };
            per_backend_per_opt
                .entry(b)
                .or_default()
                .push((opt, code));
            rows.push(Row {
                file: file.clone(),
                backend: b,
                opt,
                result: r,
            });
        }
    }

    // Disagreements: same backend, O0 vs O3, different exit codes
    // (or one passes and the other doesn't).
    let mut disagreements: Vec<String> = Vec::new();
    for b in &backends {
        if let Some(entries) = per_backend_per_opt.get(b) {
            if entries.len() == 2 {
                let (o0_opt, c0) = entries[0];
                let (o3_opt, c3) = entries[1];
                // entries were pushed in [O0, O3] order
                let _ = (o0_opt, o3_opt);
                if c0 != c3 {
                    disagreements.push(format!(
                        "{}/{} (O0={:?} vs O3={:?})",
                        backend_name(*b),
                        file,
                        c0,
                        c3
                    ));
                }
            }
        }
    }

    let total = rows.len();
    let passed = rows.iter().filter(|r| r.result.is_pass()).count();

    if json {
        let out = build_json("opt_compare", None, rows.clone(), disagreements.clone());
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        print_table(&rows);
        println!("\n=== Opt-compare summary ===");
        println!(
            "  (backend, file) pairs with O0/O3 disagreement: {}",
            disagreements.len()
        );
        for d in &disagreements {
            println!("    DIFF  {}", d);
        }
        print_summary_line(total, passed);
    }

    exit_code(passed == total, &disagreements)
}

// ── shared helpers ─────────────────────────────────────────────────────

fn exit_code(all_pass: bool, disagreements: &[String]) -> ExitCode {
    if all_pass && disagreements.is_empty() {
        ExitCode::from(0)
    } else {
        ExitCode::from(1)
    }
}

fn print_table(rows: &[Row]) {
    println!(
        "{:<48} {:<14} {:<4} {:<8} {}",
        "PROGRAM", "BACKEND", "OPT", "RESULT", "DETAIL"
    );
    println!("{}", "-".repeat(110));
    for r in rows {
        let file_short: String = r.file.chars().take(48).collect();
        println!(
            "{:<48} {:<14} {:<4} {:<8} {}",
            file_short,
            backend_name(r.backend),
            opt_name(r.opt),
            r.result.label(),
            r.result.summary()
        );
    }
}

fn print_summary_line(total: usize, passed: usize) {
    let failed = total - passed;
    println!();
    println!(
        "=== Summary: {} total, {} passed, {} failed ===",
        total, passed, failed
    );
}

fn build_json(
    mode: &str,
    opt: Option<&str>,
    rows: Vec<Row>,
    disagreements: Vec<String>,
) -> JsonOutput {
    let mut json_rows: Vec<JsonRow> = Vec::new();
    let mut total: usize = 0;
    let mut passed: usize = 0;

    for r in &rows {
        total += 1;
        if r.result.is_pass() {
            passed += 1;
        }
        json_rows.push(JsonRow {
            file: r.file.clone(),
            backend: backend_name(r.backend).to_string(),
            opt: opt_name(r.opt).to_string(),
            result: r.result.clone(),
        });
    }

    let failed = total - passed;
    JsonOutput {
        mode: mode.to_string(),
        opt: opt.map(|s| s.to_string()),
        results: json_rows,
        disagreements,
        summary: JsonSummary {
            total,
            passed,
            failed,
        },
    }
}
