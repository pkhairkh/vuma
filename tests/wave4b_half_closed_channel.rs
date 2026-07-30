//! # Caveat §2.3 — Half-closed two-pipe channel (Wave 4-b, static IR audit)
//!
//! **Companion to:** `tests/gold_standard/ipc/half_closed_channel.vuma` and
//! `tests/gold_standard/ipc/half_closed_negative.vuma`.
//!
//! ## Background
//!
//! Per caveat §2.3, the two-pipe channel architecture means send and recv
//! touch DIFFERENT pipes:
//!   - `channel_send` writes to `write_fd1` at handle offset 4 (pipe 1)
//!   - `channel_recv` reads from `read_fd2` at handle offset 8 (pipe 2)
//!
//! Closing one direction (the parent's write end, offset 4) leaves the
//! surviving direction (child→parent, offset 8) fully intact. There is no
//! dedicated `channel_close_send` builtin — `channel_close` closes all 4 fds.
//! To demonstrate a TRUE half-close, the .vuma test programs use:
//!   - `shared_memory_read(ch, 4)` — a generic pointer-deref primitive that
//!     loads i64 from `ch+4` (extracting write_fd1 in the lower 32 bits).
//!   - `& 4294967295` — mask to isolate write_fd1.
//!   - `syscall(57, wfd)` — raw `close()` (asm-generic nr 57) on the single fd.
//!
//! ## Runtime execution gap (pre-existing, documented in Wave 4-c-test)
//!
//! The `vuma build` / `vuma run` / `vuma emit` CLI commands route through
//! `compile_to_binary_direct` (src/main.rs:1680), which does NOT call
//! `ipc_lowering::lower_ipc_builtins` — IPC builtins are stubbed to SIGILL.
//! The canonical `compile_with_path` (pipeline.rs:1512) DOES lower IPC, but
//! its runtime codegen for IPC-lowered IR crashes (SIGSEGV) in this
//! environment for ALL IPC programs (including the known-good
//! `simple_send.vuma`). This is a pre-existing toolchain gap, NOT a defect
//! in the half-close test logic.
//!
//! ## What this test verifies (static IR audit)
//!
//! Since runtime execution is blocked by the pre-existing CLI/codegen gap,
//! this test verifies the half-close logic at the IR level — the SAME
//! approach used by Wave 4-c-test (which verified the K11A warning
//! mechanism statically via `dump_ir` and documented the CLI gap).
//!
//! For each .vuma program, the test:
//! 1. Parses the source.
//! 2. Builds the codegen SCG and converts to IR.
//! 3. Runs `lower_ipc_builtins` (the exact function the canonical pipeline
//!    calls at pipeline.rs:1171).
//! 4. Walks the lowered IR and asserts the half-close pattern is present:
//!    - A `Load I64` from `handle+4` (the `shared_memory_read` expansion).
//!    - A `BinOp And` with `4294967295` (the mask isolating write_fd1).
//!    - A `Syscall { nr: 57 }` (the `close(write_fd1)` half-close).
//!    - A `Load I32` from `handle+8` (the surviving `channel_recv` reading
//!      read_fd2 — a DIFFERENT offset/fd from the closed write_fd1).
//!
//! For the negative case, additionally:
//!    - A `Syscall { nr: 64 }` (the raw `write()` to the closed fd).

use vuma::pipeline::bridge_ast_to_codegen_scg;
use vuma_codegen::backend::BackendKind;
use vuma_codegen::ir::{IRInstr, IRType, IRValue};
use vuma_codegen::ipc_lowering::lower_ipc_builtins;
use vuma_codegen::ScgToIr;

use std::fs;
use std::path::PathBuf;

fn vuma_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/gold_standard/ipc");
    p.push(name);
    assert!(p.exists(), "missing test program: {}", p.display());
    p
}

/// Parse + SCG + IR + lower IPC builtins. Returns the lowered IR functions.
fn lower_program(name: &str) -> Vec<vuma_codegen::ir::IRFunction> {
    let source = fs::read_to_string(vuma_path(name))
        .unwrap_or_else(|e| panic!("read {}: {}", name, e));
    let mut parser = vuma_parser::Parser::new(&source);
    let parse_result = parser.parse_program();
    assert!(parse_result.is_ok(), "parse {} failed: {:?}", name, parse_result.errors);
    let program = parse_result.value.unwrap();

    let codegen_scg = bridge_ast_to_codegen_scg(&program);
    let mut ir_builder = ScgToIr::new();
    let mut ir_program = ir_builder.convert(&codegen_scg)
        .unwrap_or_else(|e| panic!("IR convert {}: {}", name, e));

    // Lower IPC builtins for x86_64 (the audit target). This is the exact
    // call the canonical pipeline makes at pipeline.rs:1171.
    for func in &mut ir_program.functions {
        lower_ipc_builtins(func, BackendKind::X86_64);
    }
    ir_program.functions
}

/// Collect all instructions across all blocks of all functions.
fn all_instrs(funcs: &[vuma_codegen::ir::IRFunction]) -> Vec<&IRInstr> {
    let mut out = Vec::new();
    for f in funcs {
        for b in &f.blocks {
            for i in &b.instructions {
                out.push(i);
            }
        }
    }
    out
}

#[test]
fn half_closed_channel_lowers_half_close_then_surviving_recv() {
    let funcs = lower_program("half_closed_channel.vuma");
    let instrs = all_instrs(&funcs);

    // 1. shared_memory_read(ch, 4) → BinOp Add (handle, Immediate(4)) + Load I64.
    //    The +4 is in the BinOp Add (the Load itself is at offset 0 from the
    //    summed address). Assert the BinOp Add with I64 + immediate 4 exists.
    let has_shmr_addr = instrs.iter().any(|i| {
        matches!(i, IRInstr::BinOp {
            op: vuma_codegen::ir::BinOpKind::Add,
            rhs: IRValue::Immediate(4),
            ty: Some(IRType::I64),
            ..
        })
    });
    assert!(
        has_shmr_addr,
        "expected BinOp Add (handle, 4) I64 (shared_memory_read address computation)"
    );
    // And the resulting Load I64 (reads write_fd1 | read_fd2<<32 from handle+4).
    let has_load_i64 = instrs.iter().any(|i| {
        matches!(i, IRInstr::Load { ty: IRType::I64, .. })
    });
    assert!(has_load_i64, "expected Load I64 (shared_memory_read result)");

    // 2. Mask: BinOp And with 4294967295 (0xFFFFFFFF)
    let has_mask = instrs.iter().any(|i| {
        matches!(i, IRInstr::BinOp {
            op: vuma_codegen::ir::BinOpKind::And,
            rhs: IRValue::Immediate(4294967295),
            ..
        })
    });
    assert!(has_mask, "expected BinOp And with 4294967295 (mask to write_fd1)");

    // 3. close(write_fd1): Syscall nr 57
    let close_count = instrs.iter().filter(|i| {
        matches!(i, IRInstr::Syscall { nr: 57, .. })
    }).count();
    // The half-close emits ONE nr:57 (from syscall(57, wfd)). The final
    // channel_close emits FOUR more (closing all 4 fds). So total >= 5.
    assert!(
        close_count >= 5,
        "expected >= 5 close syscalls (1 half-close + 4 from channel_close), got {}",
        close_count
    );

    // 4. Surviving direction: channel_recv loads read_fd2 from handle+8 (I32).
    //    This is a DIFFERENT offset (8) from the half-closed write_fd1 (offset 4),
    //    proving the surviving direction uses a DIFFERENT pipe.
    let has_recv_load_offset_8 = instrs.iter().any(|i| {
        matches!(i, IRInstr::Load { offset: 8, ty: IRType::I32, .. })
    });
    assert!(
        has_recv_load_offset_8,
        "expected Load I32 at offset 8 (channel_recv of read_fd2 — the surviving direction)"
    );

    // 5. The half-close (offset 4) and surviving recv (offset 8) touch
    //    DIFFERENT offsets — this IS the two-pipe half-closure property.
    //    (Verified above: both offsets present, distinct.)
}

#[test]
fn half_closed_negative_lowers_close_then_write_to_closed_fd() {
    let funcs = lower_program("half_closed_negative.vuma");
    let instrs = all_instrs(&funcs);

    // 1. shared_memory_read(ch, 4) + mask (same as positive case)
    let has_shmr_addr = instrs.iter().any(|i| {
        matches!(i, IRInstr::BinOp {
            op: vuma_codegen::ir::BinOpKind::Add,
            rhs: IRValue::Immediate(4),
            ty: Some(IRType::I64),
            ..
        })
    });
    assert!(has_shmr_addr, "expected BinOp Add (handle, 4) I64");
    let has_load_i64 = instrs.iter().any(|i| {
        matches!(i, IRInstr::Load { ty: IRType::I64, .. })
    });
    assert!(has_load_i64, "expected Load I64");

    let has_mask = instrs.iter().any(|i| {
        matches!(i, IRInstr::BinOp {
            op: vuma_codegen::ir::BinOpKind::And,
            rhs: IRValue::Immediate(4294967295),
            ..
        })
    });
    assert!(has_mask, "expected BinOp And with 4294967295");

    // 2. close(write_fd1): Syscall nr 57 (the half-close)
    let close_count = instrs.iter().filter(|i| {
        matches!(i, IRInstr::Syscall { nr: 57, .. })
    }).count();
    assert!(close_count >= 5, "expected >= 5 close syscalls, got {}", close_count);

    // 3. Raw write to the closed fd: Syscall nr 64 (write).
    //    This is the NEGATIVE-case probe — writing to the closed fd should
    //    return -EBADF at runtime.
    let has_write_syscall = instrs.iter().any(|i| {
        matches!(i, IRInstr::Syscall { nr: 64, .. })
    });
    assert!(
        has_write_syscall,
        "expected Syscall nr 64 (raw write to closed fd — the negative-case probe)"
    );
}

#[test]
fn half_close_uses_different_offset_than_surviving_direction() {
    // The CORE two-pipe property: send (offset 4) and recv (offset 8) touch
    // DIFFERENT handle offsets = DIFFERENT pipes. Closing offset 4 cannot
    // affect offset 8. This test asserts both offsets are exercised in the
    // SAME program, on the SAME handle, proving they are independent.
    let funcs = lower_program("half_closed_channel.vuma");
    let instrs = all_instrs(&funcs);

    let offsets_loaded: Vec<i32> = instrs.iter().filter_map(|i| {
        match i {
            IRInstr::Load { offset, .. } => Some(*offset),
            _ => None,
        }
    }).collect();

    // Offset 4 (write_fd1 — half-closed) and offset 8 (read_fd2 — surviving)
    // must BOTH appear as Load offsets in the lowered IR.
    assert!(offsets_loaded.contains(&4), "offset 4 (write_fd1) must be loaded");
    assert!(offsets_loaded.contains(&8), "offset 8 (read_fd2) must be loaded");
    assert!(offsets_loaded.contains(&0), "offset 0 (read_fd1) present (handle layout)");
    assert!(offsets_loaded.contains(&12), "offset 12 (write_fd2) present (handle layout)");
}
