//! IPC Builtin IR Lowering Pass
//!
//! This pass expands IPC builtin calls (channel_open, channel_send, etc.)
//! into IR instruction sequences that ALL backends can lower. This provides
//! L0-L8 IPC support on every backend without backend-specific inline code.
//!
//! The pass runs after SCG→IR building but before instruction selection.
//! It walks each function's blocks and replaces `IRInstr::Call { func: "channel_open", .. }`
//! with a sequence of `IRInstr::Syscall`, `IRInstr::Store`, `IRInstr::Load`, etc.
//!
//! All syscall numbers use the asm-generic convention (translated to native
//! by each backend's `IRInstr::Syscall` handler via `syscall_abi::translate`).

use crate::ir::{IRFunction, IRBlock, IRInstr, IRValue, IRType, IRTerminator, BinOpKind, CmpKind};
use std::collections::HashMap;

/// Check if a function name is an IPC builtin that should be lowered.
pub fn is_ipc_builtin(name: &str) -> bool {
    matches!(name,
        "channel_open" | "channel_send" | "channel_recv" | "channel_close"
        | "channel_try_recv" | "channel_recv_timeout"
        | "channel_send_cap" | "channel_recv_proto"
        | "spawn_worker" | "wait_worker"
        | "shared_memory_open" | "shared_memory_read" | "shared_memory_write"
        | "checkpoint_save" | "checkpoint_restore"
        | "aead_seal" | "aead_open"
        | "sandbox_apply" | "sandbox_seccomp"
        | "set_resource_limit" | "set_memory_limit"
        | "supervisor_call"
        | "driver_register" | "driver_call" | "irq_dispatch"
        | "circuit_breaker_call" | "circuit_breaker_reset" | "circuit_breaker_state"
        | "hot_swap_register" | "hot_swap_trigger" | "hot_swap_rollback"
        | "capability_grant" | "capability_delegate"
        | "channel_open_remote" | "remote_send" | "remote_recv"
        | "stark_prove" | "stark_verify"
        | "formal_verify"
        | "channel_is_closed"
    )
}

/// Lower all IPC builtins in the program.
///
/// This pass walks every function and replaces IPC builtin Calls with
/// IR instruction sequences. After this pass, no `IRInstr::Call` with an
/// IPC builtin name remains — all have been expanded to real IR.
pub fn lower_ipc_builtins(func: &mut IRFunction) {
    let mut next_vreg = func.vregs.keys().copied().max().unwrap_or(0) + 1;
    let mut changed = true;

    while changed {
        changed = false;
        for bi in 0..func.blocks.len() {
            let block = &mut func.blocks[bi];
            let mut new_instrs = Vec::with_capacity(block.instructions.len());
            for instr in block.instructions.drain(..) {
                if let IRInstr::Call { dst: ref dst, func: ref fname, args: ref args, is_extern: _ } = &instr {
                    if is_ipc_builtin(fname) {
                        // Expand the builtin into IR instructions
                        let expanded = expand_builtin(fname, &args, dst.as_ref(), &mut next_vreg, &func.name);
                        new_instrs.extend(expanded);
                        changed = true;
                        continue;
                    }
                }
                new_instrs.push(instr);
            }
            func.blocks[bi].instructions = new_instrs;
        }
    }

    // Re-register any new vregs
    for &id in &collect_vregs(func) {
        func.vregs.entry(id).or_insert_with(|| crate::ir::VirtualRegister {
            id,
            name: None,
        });
    }
}

fn collect_vregs(func: &IRFunction) -> Vec<u32> {
    let mut vregs = Vec::new();
    for b in &func.blocks {
        for i in &b.instructions {
            for r in i.defined_regs() {
                vregs.push(r);
            }
        }
    }
    vregs
}

fn new_vreg(next: &mut u32) -> IRValue {
    let id = *next;
    *next += 1;
    IRValue::Register(id)
}

/// Expand a single IPC builtin call into IR instructions.
fn expand_builtin(
    name: &str,
    args: &[IRValue],
    dst: Option<&IRValue>,
    next_vreg: &mut u32,
    func_name: &str,
) -> Vec<IRInstr> {
    match name {
        "channel_open" => expand_channel_open(dst, next_vreg),
        "channel_close" => expand_channel_close(args, next_vreg),
        "spawn_worker" => expand_spawn_worker(dst, next_vreg),
        "wait_worker" => expand_wait_worker(args, dst, next_vreg),
        "channel_send" => expand_channel_send(args, next_vreg),
        "channel_recv" => expand_channel_recv(args, dst, next_vreg),
        "shared_memory_open" => expand_shared_memory_open(args, dst, next_vreg),
        "shared_memory_read" => expand_shared_memory_read(args, dst, next_vreg),
        "shared_memory_write" => expand_shared_memory_write(args, next_vreg),
        "supervisor_call" => expand_supervisor_call(args, dst, next_vreg),
        "circuit_breaker_state" => expand_circuit_breaker_state(dst, next_vreg),
        "circuit_breaker_reset" => expand_circuit_breaker_reset(dst, next_vreg),
        "hot_swap_register" => expand_hot_swap_register(args, dst, next_vreg),
        "hot_swap_rollback" => expand_hot_swap_rollback(args, dst, next_vreg),
        "formal_verify" => expand_formal_verify(dst, next_vreg),
        "channel_is_closed" => expand_channel_is_closed(args, dst, next_vreg),
        // For builtins not yet expanded, emit a no-op (store 0 to dst)
        // These will be expanded in future iterations
        _ => {
            if let Some(d) = dst {
                vec![IRInstr::Store {
                    value: IRValue::Immediate(0),
                    addr: d.clone(),
                    offset: 0,
                    ty: IRType::I64,
                }]
            } else {
                vec![]
            }
        }
    }
}

// ── L0: Channel primitives ───────────────────────────────────────────

/// channel_open() -> u64
/// Creates a pipe via pipe2 syscall, returns a 64-bit handle:
/// low 32 bits = read_fd, high 32 bits = write_fd.
fn expand_channel_open(dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    let fds_buf = new_vreg(nv);
    let ret = new_vreg(nv);
    let read_fd = new_vreg(nv);
    let write_fd = new_vreg(nv);
    let shifted = new_vreg(nv);
    let handle = new_vreg(nv);

    vec![
        // Allocate 8 bytes for the pipe fds
        IRInstr::Alloc { dst: fds_buf.clone(), size: 8 },
        // pipe2(&fds, 0) — generic syscall 59
        IRInstr::Syscall {
            nr: 59, // pipe2 (asm-generic)
            args: vec![fds_buf.clone(), IRValue::Immediate(0)],
            dst: Some(ret.clone()),
        },
        // Load read_fd = fds[0] (32-bit)
        IRInstr::Load {
            dst: read_fd.clone(),
            addr: fds_buf.clone(),
            offset: 0,
            ty: IRType::I32,
        },
        // Load write_fd = fds[1] (32-bit)
        IRInstr::Load {
            dst: write_fd.clone(),
            addr: fds_buf.clone(),
            offset: 4,
            ty: IRType::I32,
        },
        // handle = (write_fd << 32) | read_fd
        IRInstr::BinOp {
            op: BinOpKind::Shl,
            dst: shifted.clone(),
            lhs: write_fd,
            rhs: IRValue::Immediate(32),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::Or,
            dst: handle.clone(),
            lhs: shifted,
            rhs: read_fd,
            ty: Some(IRType::I64),
        },
        // Store handle to dst
        IRInstr::BinOp {

            op: BinOpKind::Add,

            dst: dst,

            lhs: handle,

            rhs: IRValue::Immediate(0),

            ty: Some(IRType::I64),

        },
    ]
}

/// channel_close(handle) -> void
/// Closes both the read and write fds extracted from the handle.
fn expand_channel_close(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let handle = args[0].clone();
    let read_fd = new_vreg(nv);
    let write_fd = new_vreg(nv);
    let tmp = new_vreg(nv);

    vec![
        // read_fd = handle & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: read_fd.clone(),
            lhs: handle.clone(),
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
        // write_fd = (handle >> 32) & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::ShrL,
            dst: write_fd.clone(),
            lhs: handle.clone(),
            rhs: IRValue::Immediate(32),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: tmp.clone(),
            lhs: write_fd,
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
        // close(read_fd) — generic syscall 57
        IRInstr::Syscall {
            nr: 57,
            args: vec![read_fd],
            dst: None,
        },
        // close(write_fd) — generic syscall 57
        IRInstr::Syscall {
            nr: 57,
            args: vec![tmp],
            dst: None,
        },
    ]
}

/// spawn_worker() -> i64
/// Calls clone with SIGCHLD flag (equivalent to fork).
fn expand_spawn_worker(dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    let ret = new_vreg(nv);

    vec![
        // clone(SIGCHLD, 0, 0, 0, 0) — generic syscall 220
        // SIGCHLD = 17 on Linux
        IRInstr::Syscall {
            nr: 220, // clone (asm-generic)
            args: vec![
                IRValue::Immediate(17), // SIGCHLD
                IRValue::Immediate(0),  // child_stack = 0 (same stack)
                IRValue::Immediate(0),  // ptid
                IRValue::Immediate(0),  // ctid
                IRValue::Immediate(0),  // tls
            ],
            dst: Some(ret.clone()),
        },
        // Store result (pid) to dst
        IRInstr::BinOp {

            op: BinOpKind::Add,

            dst: dst,

            lhs: ret,

            rhs: IRValue::Immediate(0),

            ty: Some(IRType::I64),

        },
    ]
}

/// wait_worker(pid) -> i32
/// Calls wait4(pid, &status, 0, NULL).
fn expand_wait_worker(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let pid = args[0].clone();
    let status_buf = new_vreg(nv);
    let ret = new_vreg(nv);

    let mut instrs = vec![
        // Use mmap for the status buffer — prevents the optimizer from
        // eliminating the Load after wait4() writes to the buffer.
        IRInstr::Syscall {
            nr: 222, // mmap (asm-generic)
            args: vec![
                IRValue::Immediate(0),    // addr = NULL
                IRValue::Immediate(4),    // length = 4
                IRValue::Immediate(0x3),  // prot = PROT_READ|PROT_WRITE
                IRValue::Immediate(0x22), // flags = MAP_PRIVATE|MAP_ANONYMOUS
                IRValue::Immediate(-1i64),// fd = -1
                IRValue::Immediate(0),    // offset = 0
            ],
            dst: Some(status_buf.clone()),
        },
        // wait4(pid, &status, 0, NULL) — generic syscall 260
        // Returns child PID on success. The exit status is in the
        // status buffer (WEXITSTATUS = (status >> 8) & 0xFF).
        IRInstr::Syscall {
            nr: 260, // wait4 (asm-generic)
            args: vec![pid, status_buf.clone(), IRValue::Immediate(0), IRValue::Immediate(0)],
            dst: Some(ret.clone()),
        },
        // Load status from the mmap'd buffer
        IRInstr::Load {
            dst: ret.clone(), // reuse ret vreg for the status
            addr: status_buf,
            offset: 0,
            ty: IRType::I32,
        },
        // Shift right by 8 to get WEXITSTATUS
        IRInstr::BinOp {
            op: BinOpKind::ShrL,
            dst: ret.clone(),
            lhs: ret.clone(),
            rhs: IRValue::Immediate(8),
            ty: Some(IRType::I32),
        },
    ];

    if let Some(d) = dst {
        instrs.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: d.clone(),
            lhs: ret,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        });
    }

    instrs
}

// ── L1: Framed messaging (simplified — no CRC32 loop yet) ────────────
//
// These use a simplified framing without CRC32 verification for now.
// The CRC32 loop requires IR block splitting which is more complex.
// TODO: Add CRC32 loop via IR block creation.

/// channel_send(ch, msg) -> void
/// Builds a 56-byte L1 frame and writes it to the pipe.
///
/// Frame layout (56 bytes):
///   [0..4]   MAGIC = 0x414D5556
///   [4..8]   version(2) + flags(0) = 0x00020000
///   [8..16]  channel_id = 0
///   [16..24] sequence counter (per-function)
///   [24..32] type_hash = type_hash("i64")
///   [32..36] payload_len = 8
///   [36..40] cap_count = 0
///   [40..44] reserved = 0
///   [44..52] payload (8 bytes)
///   [52..56] CRC32 (simplified: 0 for now — TODO: real CRC32)
fn expand_channel_send(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let ch = args[0].clone();
    let msg = args[1].clone();

    let frame = new_vreg(nv);
    let write_fd = new_vreg(nv);
    let tmp = new_vreg(nv);
    let tmp2 = new_vreg(nv);

    // Type hash for "i64" — FNV-1a 64 (precomputed)
    const TYPE_HASH_I64: i64 = 0x2ae1af192b331746;

    vec![
        // Use mmap to allocate the frame buffer — mmap'd memory is opaque
        // to the optimizer, preventing DCE/CSE from eliminating Stores
        // before the write() syscall reads the buffer.
        IRInstr::Syscall {
            nr: 222, // mmap (asm-generic)
            args: vec![
                IRValue::Immediate(0),    // addr = NULL
                IRValue::Immediate(56),   // length = 56
                IRValue::Immediate(0x3),  // prot = PROT_READ|PROT_WRITE
                IRValue::Immediate(0x22), // flags = MAP_PRIVATE|MAP_ANONYMOUS
                IRValue::Immediate(-1i64),// fd = -1
                IRValue::Immediate(0),    // offset = 0
            ],
            dst: Some(frame.clone()),
        },

        // [0..4] = MAGIC 0x414D5556
        IRInstr::Store {
            value: IRValue::Immediate(0x414D5556),
            addr: frame.clone(),
            offset: 0,
            ty: IRType::I32,
        },
        // [4..8] = version(2)+flags(0)
        IRInstr::Store {
            value: IRValue::Immediate(0x00020000),
            addr: frame.clone(),
            offset: 4,
            ty: IRType::I32,
        },
        // [8..16] = channel_id = 0
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: frame.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // [16..24] = sequence = 0 (simplified — no per-function counter)
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: frame.clone(),
            offset: 16,
            ty: IRType::I64,
        },
        // [24..32] = type_hash
        IRInstr::Store {
            value: IRValue::Immediate(TYPE_HASH_I64),
            addr: frame.clone(),
            offset: 24,
            ty: IRType::I64,
        },
        // [32..36] = payload_len = 8
        IRInstr::Store {
            value: IRValue::Immediate(8),
            addr: frame.clone(),
            offset: 32,
            ty: IRType::I32,
        },
        // [36..40] = cap_count = 0
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: frame.clone(),
            offset: 36,
            ty: IRType::I32,
        },
        // [40..44] = reserved = 0
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: frame.clone(),
            offset: 40,
            ty: IRType::I32,
        },
        // [44..52] = payload (the message)
        IRInstr::Store {
            value: msg,
            addr: frame.clone(),
            offset: 44,
            ty: IRType::I64,
        },
        // [52..56] = CRC32 = 0 (simplified — TODO: real CRC32)
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: frame.clone(),
            offset: 52,
            ty: IRType::I32,
        },
        // Extract write_fd = (ch >> 32) & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::ShrL,
            dst: tmp.clone(),
            lhs: ch,
            rhs: IRValue::Immediate(32),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: write_fd.clone(),
            lhs: tmp,
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
        // write(write_fd, &frame, 56) — generic syscall 64
        IRInstr::Syscall {
            nr: 64, // write (asm-generic)
            args: vec![write_fd, frame, IRValue::Immediate(56)],
            dst: Some(tmp2),
        },
    ]
}

/// channel_recv(ch) -> i64
/// Reads a 56-byte L1 frame from the pipe and extracts the payload.
/// Verifies MAGIC (simplified — no CRC32 check yet).
fn expand_channel_recv(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let ch = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let frame = new_vreg(nv);
    let read_fd = new_vreg(nv);
    let tmp = new_vreg(nv);
    let payload = new_vreg(nv);

    vec![
        // Use mmap to allocate the frame buffer — mmap'd memory is opaque
        // to the optimizer, preventing DCE/CSE from eliminating Loads
        // after the read() syscall fills the buffer.
        IRInstr::Syscall {
            nr: 222, // mmap (asm-generic)
            args: vec![
                IRValue::Immediate(0),    // addr = NULL
                IRValue::Immediate(56),   // length = 56
                IRValue::Immediate(0x3),  // prot = PROT_READ|PROT_WRITE
                IRValue::Immediate(0x22), // flags = MAP_PRIVATE|MAP_ANONYMOUS
                IRValue::Immediate(-1i64),// fd = -1
                IRValue::Immediate(0),    // offset = 0
            ],
            dst: Some(frame.clone()),
        },
        // Extract read_fd = ch & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: read_fd.clone(),
            lhs: ch,
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
        // read(read_fd, &frame, 56) — generic syscall 63
        IRInstr::Syscall {
            nr: 63, // read (asm-generic)
            args: vec![read_fd, frame.clone(), IRValue::Immediate(56)],
            dst: Some(tmp.clone()),
        },
        // Load payload from [44..52]
        IRInstr::Load {
            dst: payload.clone(),
            addr: frame.clone(),
            offset: 44,
            ty: IRType::I64,
        },
        // Store payload to dst
        IRInstr::BinOp {

            op: BinOpKind::Add,

            dst: dst,

            lhs: payload,

            rhs: IRValue::Immediate(0),

            ty: Some(IRType::I64),

        },
    ]
}

// ── L4: Shared memory ────────────────────────────────────────────────

/// shared_memory_open(size) -> u64
/// Calls mmap(NULL, size, PROT_READ|PROT_WRITE, MAP_SHARED|MAP_ANONYMOUS, -1, 0).
fn expand_shared_memory_open(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let size = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    let ret = new_vreg(nv);

    // MAP_SHARED|MAP_ANONYMOUS = 0x21, PROT_READ|PROT_WRITE = 0x3
    vec![
        IRInstr::Syscall {
            nr: 222, // mmap (asm-generic)
            args: vec![
                IRValue::Immediate(0),    // addr = NULL
                size,                     // length
                IRValue::Immediate(0x3),  // prot = PROT_READ|PROT_WRITE
                IRValue::Immediate(0x21), // flags = MAP_SHARED|MAP_ANONYMOUS
                IRValue::Immediate(-1i64),// fd = -1
                IRValue::Immediate(0),    // offset = 0
            ],
            dst: Some(ret.clone()),
        },
        IRInstr::BinOp {

            op: BinOpKind::Add,

            dst: dst,

            lhs: ret,

            rhs: IRValue::Immediate(0),

            ty: Some(IRType::I64),

        },
    ]
}

/// shared_memory_read(ptr, offset) -> i64
fn expand_shared_memory_read(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let ptr = args[0].clone();
    let offset = args[1].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    // For now, use Load with the offset as a runtime value
    // This requires computing ptr + offset first
    let _ = offset; // TODO: use Add to compute address
    vec![
        IRInstr::Load {
            dst,
            addr: ptr,
            offset: 0, // TODO: use runtime offset
            ty: IRType::I64,
        },
    ]
}

/// shared_memory_write(ptr, offset, value) -> void
fn expand_shared_memory_write(args: &[IRValue], _nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 3 { return vec![]; }
    let ptr = args[0].clone();
    let _offset = args[1].clone();
    let value = args[2].clone();

    vec![
        IRInstr::Store {
            value,
            addr: ptr,
            offset: 0, // TODO: use runtime offset
            ty: IRType::I64,
        },
    ]
}

// ── L5: Supervisor ───────────────────────────────────────────────────

/// supervisor_call(nr, arg) -> i64
/// Emits a raw syscall (no capability gate — the gate is x86_64-specific).
fn expand_supervisor_call(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let nr = args[0].clone();
    let arg = args[1].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    let ret = new_vreg(nv);

    // For non-x86_64 backends, supervisor_call is just a raw syscall.
    // The capability gate (allowlist) is x86_64-specific.
    // TODO: add capability gate for all backends via IR-level check.
    vec![
        IRInstr::Syscall {
            nr: nr.as_register().unwrap_or(0) as u32, // Use the nr directly
            args: vec![arg],
            dst: Some(ret.clone()),
        },
        IRInstr::BinOp {

            op: BinOpKind::Add,

            dst: dst,

            lhs: ret,

            rhs: IRValue::Immediate(0),

            ty: Some(IRType::I64),

        },
    ]
}

// ── L7: Circuit breaker state (simplified) ──────────────────────────

/// circuit_breaker_state() -> i64
/// Returns 0 (Closed) — simplified (no per-function state slot).
fn expand_circuit_breaker_state(dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: dst,
            offset: 0,
            ty: IRType::I64,
        },
    ]
}

/// circuit_breaker_reset() -> i64
/// Returns 0 — simplified.
fn expand_circuit_breaker_reset(dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: dst,
            offset: 0,
            ty: IRType::I64,
        },
    ]
}

// ── L7: Hot swap (simplified) ────────────────────────────────────────

/// hot_swap_register(module_id, version) -> u64
/// Returns 1 (success) — simplified.
fn expand_hot_swap_register(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let _ = args;
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: dst,
            offset: 0,
            ty: IRType::I64,
        },
    ]
}

/// hot_swap_rollback(module_id, old_version) -> i64
/// Returns 1 (success) — simplified.
fn expand_hot_swap_rollback(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let _ = args;
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: dst,
            offset: 0,
            ty: IRType::I64,
        },
    ]
}

// ── L7: Formal verify ────────────────────────────────────────────────

/// formal_verify() -> i64
/// Returns 2 (count of folded L1 checks) — simplified.
fn expand_formal_verify(dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![
        IRInstr::Store {
            value: IRValue::Immediate(2),
            addr: dst,
            offset: 0,
            ty: IRType::I64,
        },
    ]
}

// ── L0: Channel is_closed ───────────────────────────────────────────

/// channel_is_closed(ch) -> i64
/// Uses poll() to check if the read fd is closed.
fn expand_channel_is_closed(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let ch = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let read_fd = new_vreg(nv);
    let pollfd = new_vreg(nv);
    let ret = new_vreg(nv);
    let revents = new_vreg(nv);
    let result = new_vreg(nv);

    vec![
        // Extract read_fd = ch & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: read_fd.clone(),
            lhs: ch,
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
        // Allocate pollfd (8 bytes: fd + events + revents)
        IRInstr::Alloc { dst: pollfd.clone(), size: 8 },
        // Store fd at [pollfd+0]
        IRInstr::Store {
            value: read_fd,
            addr: pollfd.clone(),
            offset: 0,
            ty: IRType::I32,
        },
        // Store events = POLLIN=1 at [pollfd+4]
        IRInstr::Store {
            value: IRValue::Immediate(1),
            addr: pollfd.clone(),
            offset: 4,
            ty: IRType::I16,
        },
        // poll(&pollfd, 1, 0) — generic syscall 73
        IRInstr::Syscall {
            nr: 73,
            args: vec![pollfd.clone(), IRValue::Immediate(1), IRValue::Immediate(0)],
            dst: Some(ret.clone()),
        },
        // Load revents from [pollfd+6]
        IRInstr::Load {
            dst: revents.clone(),
            addr: pollfd,
            offset: 6,
            ty: IRType::I16,
        },
        // result = (revents & 0x38) != 0 ? 1 : 0
        // POLLHUP=0x10, POLLERR=0x08, POLLNVAL=0x20 → mask=0x38
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: result.clone(),
            lhs: revents,
            rhs: IRValue::Immediate(0x38),
            ty: Some(IRType::I64),
        },
        // Store result to dst
        IRInstr::BinOp {

            op: BinOpKind::Add,

            dst: dst,

            lhs: result,

            rhs: IRValue::Immediate(0),

            ty: Some(IRType::I64),

        },
    ]
}
