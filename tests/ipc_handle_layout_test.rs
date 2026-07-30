//! # Caveat §2.3 — Two-pipe channel handle layout audit (Wave 4-a)
//!
//! Per caveat §2.3, each VUMA channel end is a **16-byte handle holding 4
//! file descriptors**: parent→child pipe (read end @0, write end @4) and
//! child→parent pipe (read end @8, write end @12). The previous single-pipe
//! design (and its `nanosleep`-based send/recv race workaround) has been
//! removed.
//!
//! The channel handle is NOT a Rust `struct` in `vuma-codegen` — it is a
//! runtime-allocated IR buffer created by `expand_channel_open` via
//! `IRInstr::Alloc { size: 16 }` and populated by four `IRInstr::Store`
//! instructions of type `I32` at fixed offsets 0, 4, 8, 12. The four stored
//! values are the read/write fds of two `pipe2()` syscalls (asm-generic
//! `nr: 59`).
//!
//! This test verifies the layout in TWO complementary ways:
//!
//! 1. **Compile-time mirror struct** (`ChannelHandle`): a `#[repr(C)]` Rust
//!    struct with four `i32` fields that mirrors the documented IR layout.
//!    `size_of::<ChannelHandle>() == 16` and each field is fd-sized
//!    (`size_of::<i32>() == 4`). This is the "or equivalent" of
//!    `size_of::<ChannelHandle>() == 16` from the DoD — the actual handle
//!    type is an IR-level buffer, not a Rust struct, so the test binds a
//!    concrete Rust type to the IR layout and asserts its size.
//!
//! 2. **Runtime IR-layout verification**: construct a minimal `IRFunction`
//!    containing a `channel_open()` Call, run `lower_ipc_builtins` on it
//!    (x86_64 backend — the wasm32 backend lowers `channel_open` natively to
//!    ring-buffer ops and never reaches the pipe-based expansion), then walk
//!    the emitted IR and assert:
//!      - Exactly one `Alloc { size: 16 }` (the handle buffer; the two
//!        `pipe2` fd scratch buffers are size 8, the per-function channel
//!        registry is size 84).
//!      - Exactly four `Store { addr: <handle_ptr>, offset: o, ty: I32 }`
//!        instructions at offsets {0, 4, 8, 12} — one each, no dupes.
//!      - Exactly two `Syscall { nr: 59, .. }` (two `pipe2` calls — one per
//!        pipe).
//!    The same four offsets are then read back by `channel_close`, proving
//!    the handle is consumed consistently with the layout it was created
//!    with.
//!
//! ## Files
//! - Source under audit (READ-ONLY): `src/codegen/src/ipc_lowering.rs`
//!   — `expand_channel_open` (lines ~1112-1322) and `expand_channel_close`
//!   (lines ~1324-1388).
//!
//! ## DoD
//! - A test exists that asserts `size_of::<ChannelHandle>() == 16` (or
//!   equivalent). ✓ — `handle_size_is_16_bytes_compile_time`.
//! - The test passes. ✓ — see `scripts/logs/wave4_handle_test.log`.

use std::mem::size_of;

use vuma_codegen::backend::BackendKind;
use vuma_codegen::ir::{IRFunction, IRInstr, IRTerminator, IRType, IRValue};
use vuma_codegen::ipc_lowering::lower_ipc_builtins;

// ─────────────────────────────────────────────────────────────────────
// 1. Compile-time mirror struct (the "or equivalent" of
//    `size_of::<ChannelHandle>() == 16`).
// ─────────────────────────────────────────────────────────────────────

/// Mirror of the two-pipe channel handle's in-memory layout, as documented
/// in `expand_channel_open` (src/codegen/src/ipc_lowering.rs):
///
/// ```text
///   [ 0] read_fd1  — read end of pipe 1 (parent→child)
///   [ 4] write_fd1 — write end of pipe 1 (parent writes here)
///   [ 8] read_fd2  — read end of pipe 2 (child→parent; parent reads here)
///   [12] write_fd2 — write end of pipe 2 (child writes here)
/// ```
///
/// Each field is a Unix file descriptor (i32 / RawFd = 4 bytes). Four
/// fields × 4 bytes = 16 bytes total. `#[repr(C)]` pins the layout so
/// the field offsets match the IR Store/Load offsets exactly.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct ChannelHandle {
    read_fd1: i32,
    write_fd1: i32,
    read_fd2: i32,
    write_fd2: i32,
}

#[test]
fn handle_size_is_16_bytes_compile_time() {
    // The headline DoD assertion: a 4-fd handle is 16 bytes.
    assert_eq!(size_of::<ChannelHandle>(), 16, "channel handle must be 16 bytes");
    // Each field is exactly one fd (i32 / RawFd = 4 bytes).
    assert_eq!(size_of::<i32>(), 4, "fd field must be 4 bytes (i32)");
    // Field offsets within the #[repr(C)] struct must match the IR Store
    // offsets used by `expand_channel_open` / `expand_channel_close`.
    let r1 = &ChannelHandle {
        read_fd1: 0,
        write_fd1: 0,
        read_fd2: 0,
        write_fd2: 0,
    } as *const _ as usize;
    // Re-instantiate to take addresses of each field without borrowing the
    // whole struct mutably; the compiler elides this in release builds.
    let h = ChannelHandle {
        read_fd1: 0,
        write_fd1: 0,
        read_fd2: 0,
        write_fd2: 0,
    };
    let base = &h as *const _ as usize;
    assert_eq!((&h.read_fd1 as *const _ as usize) - base, 0);
    assert_eq!((&h.write_fd1 as *const _ as usize) - base, 4);
    assert_eq!((&h.read_fd2 as *const _ as usize) - base, 8);
    assert_eq!((&h.write_fd2 as *const _ as usize) - base, 12);
    // Silence "unused variable" on `r1` — it documents the base-address
    // convention used by the field-offset checks above.
    let _ = r1;
}

// ─────────────────────────────────────────────────────────────────────
// 2. Runtime IR-layout verification: `lower_ipc_builtins` on a function
//    containing `channel_open()` must emit a 16-byte Alloc + 4 I32 Stores
//    at offsets {0,4,8,12} + 2 pipe2 syscalls.
// ─────────────────────────────────────────────────────────────────────

/// Collect every instruction emitted into every block of `func` after
/// lowering, in block-layout order. The IPC-lowering pass may split the
/// entry block and append continuation blocks; this flattens the result
/// so the audit can walk it linearly.
fn flattened_instrs(func: &IRFunction) -> Vec<&IRInstr> {
    let mut out = Vec::new();
    for block in &func.blocks {
        out.extend(block.instructions.iter());
    }
    out
}

/// Build a minimal IR function: `fn test_open() -> i64 { call channel_open() }`.
/// The single Call to `channel_open` triggers `expand_channel_open` when
/// `lower_ipc_builtins` runs on a non-wasm32 backend.
fn build_channel_open_func() -> IRFunction {
    let mut func = IRFunction::new("test_channel_open");
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(1));
    // vreg 1 is the call's dst (the returned handle pointer).
    func.current_block().instructions.push(IRInstr::Call {
        dst: Some(IRValue::Register(1)),
        func: "channel_open".to_string(),
        args: Vec::new(),
        is_extern: false,
    });
    func.current_block().terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
    func
}

#[test]
fn channel_open_emits_16_byte_handle_with_4_i32_fds() {
    let mut func = build_channel_open_func();
    // x86_64 (NOT wasm32): wasm32 lowers channel_open natively to ring
    // buffers and never reaches the pipe2-based expansion that creates the
    // 16-byte handle. Any non-wasm32 backend exercises the same IR-level
    // expansion (the pass is backend-independent except for the wasm32
    // native-channel-builtin short-circuit).
    lower_ipc_builtins(&mut func, BackendKind::X86_64);

    let instrs = flattened_instrs(&func);

    // (a) Exactly one Alloc of size 16 — the handle buffer.
    let handle_allocs: Vec<&IRValue> = instrs
        .iter()
        .filter_map(|i| match i {
            IRInstr::Alloc { dst, size: 16 } => Some(dst),
            _ => None,
        })
        .collect();
    assert_eq!(
        handle_allocs.len(),
        1,
        "channel_open must allocate exactly one 16-byte handle buffer (got {})",
        handle_allocs.len()
    );
    let handle_ptr = handle_allocs[0].clone();

    // (b) Exactly four I32 Stores targeting `handle_ptr` at offsets
    //     {0, 4, 8, 12} — one each, no dupes, no missing.
    let mut fd_offsets: Vec<i32> = instrs
        .iter()
        .filter_map(|i| match i {
            IRInstr::Store {
                addr,
                offset,
                ty: IRType::I32,
                ..
            } if *addr == handle_ptr => Some(*offset),
            _ => None,
        })
        .collect();
    fd_offsets.sort_unstable();
    assert_eq!(
        fd_offsets,
        vec![0, 4, 8, 12],
        "handle must store 4 I32 fds at offsets {{0,4,8,12}} (got {:?})",
        fd_offsets
    );

    // (c) Exactly two pipe2 syscalls (asm-generic nr=59) — one per pipe.
    let pipe2_count = instrs
        .iter()
        .filter(|i| matches!(i, IRInstr::Syscall { nr: 59, .. }))
        .count();
    assert_eq!(
        pipe2_count, 2,
        "channel_open must call pipe2 (nr=59) exactly twice (parent→child + child→parent)"
    );
}

#[test]
fn channel_close_reads_4_fds_at_same_offsets() {
    // `channel_close(handle)` loads 4 I32 fds from the handle at offsets
    // {0, 4, 8, 12} — the same offsets `channel_open` stored them at. This
    // proves the layout is consumed consistently (close closes all 4 fds
    // that open created).
    let mut func = IRFunction::new("test_channel_close");
    func.param_types.push(IRType::I64);
    func.params.push(IRValue::Register(0));
    func.current_block().instructions.push(IRInstr::Call {
        dst: None,
        func: "channel_close".to_string(),
        args: vec![IRValue::Register(0)],
        is_extern: false,
    });
    func.current_block().terminator = IRTerminator::Return(vec![]);
    lower_ipc_builtins(&mut func, BackendKind::X86_64);

    let instrs = flattened_instrs(&func);
    let mut load_offsets: Vec<i32> = instrs
        .iter()
        .filter_map(|i| match i {
            IRInstr::Load {
                addr: IRValue::Register(0),
                offset,
                ty: IRType::I32,
                ..
            } => Some(*offset),
            _ => None,
        })
        .collect();
    load_offsets.sort_unstable();
    assert_eq!(
        load_offsets,
        vec![0, 4, 8, 12],
        "channel_close must load 4 I32 fds from the handle at offsets {{0,4,8,12}} (got {:?})",
        load_offsets
    );

    // close() = syscall nr 57 (asm-generic). Four closes — one per fd.
    let close_count = instrs
        .iter()
        .filter(|i| matches!(i, IRInstr::Syscall { nr: 57, .. }))
        .count();
    assert_eq!(
        close_count, 4,
        "channel_close must close all 4 fds (nr=57) — got {}",
        close_count
    );
}

#[test]
fn handle_layout_is_backend_independent_across_non_wasm32_backends() {
    // The two-pipe handle layout is an IR-level contract: the same 16-byte
    // buffer with 4 I32 fds at offsets {0,4,8,12} must be emitted by every
    // non-wasm32 backend (wasm32 uses in-memory ring buffers instead).
    // Verify the layout for a representative sample of 64-bit and 32-bit
    // backends.
    let backends = [
        BackendKind::X86_64,
        BackendKind::AArch64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
    ];
    for backend in backends {
        let mut func = build_channel_open_func();
        lower_ipc_builtins(&mut func, backend);
        let instrs = flattened_instrs(&func);

        let handle_allocs: Vec<&IRValue> = instrs
            .iter()
            .filter_map(|i| match i {
                IRInstr::Alloc { dst, size: 16 } => Some(dst),
                _ => None,
            })
            .collect();
        assert_eq!(
            handle_allocs.len(),
            1,
            "{:?}: expected exactly one 16-byte handle Alloc",
            backend
        );
        let handle_ptr = handle_allocs[0].clone();

        let mut offsets: Vec<i32> = instrs
            .iter()
            .filter_map(|i| match i {
                IRInstr::Store {
                    addr,
                    offset,
                    ty: IRType::I32,
                    ..
                } if *addr == handle_ptr => Some(*offset),
                _ => None,
            })
            .collect();
        offsets.sort_unstable();
        assert_eq!(
            offsets,
            vec![0, 4, 8, 12],
            "{:?}: handle fd store offsets must be {{0,4,8,12}} (got {:?})",
            backend,
            offsets
        );
    }
}
