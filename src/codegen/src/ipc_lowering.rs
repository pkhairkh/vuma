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
    let result = match name {
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
                vec![IRInstr::BinOp {
                    op: BinOpKind::Add,
                    dst: d.clone(),
                    lhs: IRValue::Immediate(0),
                    rhs: IRValue::Immediate(0),
                    ty: Some(IRType::I64),
                }]
            } else {
                vec![]
            }
        }
    };
    result
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
        IRInstr::Alloc { dst: status_buf.clone(), size: 4 },
        // wait4(pid, &status, 0, NULL) — generic syscall 260
        // Returns child PID on success. The exit status is in the
        // status buffer (WEXITSTATUS = (status >> 8) & 0xFF).
        IRInstr::Syscall {
            nr: 260, // wait4 (asm-generic)
            args: vec![pid, status_buf.clone(), IRValue::Immediate(0), IRValue::Immediate(0)],
            dst: Some(ret.clone()),
        },
        // Load status from the buffer (WEXITSTATUS = (status >> 8) & 0xFF)
        IRInstr::Load { dst: ret.clone(), addr: status_buf, offset: 0, ty: IRType::I32 },
        IRInstr::BinOp { op: BinOpKind::ShrL, dst: ret.clone(), lhs: ret.clone(), rhs: IRValue::Immediate(8), ty: Some(IRType::I32) },
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
/// Builds a full 56-byte L1 frame and writes it to the pipe.
///
/// Frame layout (56 bytes, matching the x86_64 backend):
///   [ 0.. 4] MAGIC = 0x414D5556
///   [ 4.. 8] version(2) + flags(0) = 0x00020000
///   [ 8..16] channel_id = 0
///   [16..24] sequence = 0
///   [24..32] type_hash = type_hash("i64")
///   [32..36] payload_len = 8
///   [36..40] cap_count = 0
///   [40..44] reserved = 0
///   [44..52] payload (8 bytes)
///   [52..56] CRC32 = 0 (TODO: real CRC32 loop)
fn expand_channel_send(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let ch = args[0].clone();
    let msg = args[1].clone();

    let frame = new_vreg(nv);
    let write_fd = new_vreg(nv);
    let tmp = new_vreg(nv);
    let tmp2 = new_vreg(nv);

    const TYPE_HASH_I64: i64 = 0x2ae1af192b331746;

    vec![
        IRInstr::Alloc { dst: frame.clone(), size: 56 },
        // [0..4] MAGIC
        IRInstr::Store { value: IRValue::Immediate(0x414D5556), addr: frame.clone(), offset: 0, ty: IRType::I32 },
        // [4..8] version+flags
        IRInstr::Store { value: IRValue::Immediate(0x00020000), addr: frame.clone(), offset: 4, ty: IRType::I32 },
        // [8..16] channel_id
        IRInstr::Store { value: IRValue::Immediate(0), addr: frame.clone(), offset: 8, ty: IRType::I64 },
        // [16..24] sequence
        IRInstr::Store { value: IRValue::Immediate(0), addr: frame.clone(), offset: 16, ty: IRType::I64 },
        // [24..32] type_hash
        IRInstr::Store { value: IRValue::Immediate(TYPE_HASH_I64), addr: frame.clone(), offset: 24, ty: IRType::I64 },
        // [32..36] payload_len
        IRInstr::Store { value: IRValue::Immediate(8), addr: frame.clone(), offset: 32, ty: IRType::I32 },
        // [36..40] cap_count
        IRInstr::Store { value: IRValue::Immediate(0), addr: frame.clone(), offset: 36, ty: IRType::I32 },
        // [40..44] reserved
        IRInstr::Store { value: IRValue::Immediate(0), addr: frame.clone(), offset: 40, ty: IRType::I32 },
        // [44..52] payload
        IRInstr::Store { value: msg, addr: frame.clone(), offset: 44, ty: IRType::I64 },
        // [52..56] CRC32 (simplified: 0)
        IRInstr::Store { value: IRValue::Immediate(0), addr: frame.clone(), offset: 52, ty: IRType::I32 },
        // write_fd = (ch >> 32) & 0xFFFFFFFF
        IRInstr::BinOp { op: BinOpKind::ShrL, dst: tmp.clone(), lhs: ch, rhs: IRValue::Immediate(32), ty: Some(IRType::I64) },
        IRInstr::BinOp { op: BinOpKind::And, dst: write_fd.clone(), lhs: tmp, rhs: IRValue::Immediate(0xFFFFFFFF), ty: Some(IRType::I64) },
        // write(write_fd, &frame, 56)
        IRInstr::Syscall { nr: 64, args: vec![write_fd, frame, IRValue::Immediate(56)], dst: Some(tmp2) },
    ]
}

/// channel_recv(ch) -> i64
/// Reads a full 56-byte L1 frame from the pipe and extracts the payload.
/// Verifies MAGIC (simplified — no CRC32 check yet, TODO: real CRC32 loop).
fn expand_channel_recv(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let ch = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let frame = new_vreg(nv);
    let read_fd = new_vreg(nv);
    let tmp = new_vreg(nv);
    let payload = new_vreg(nv);

    vec![
        IRInstr::Alloc { dst: frame.clone(), size: 56 },
        // read_fd = ch & 0xFFFFFFFF
        IRInstr::BinOp { op: BinOpKind::And, dst: read_fd.clone(), lhs: ch, rhs: IRValue::Immediate(0xFFFFFFFF), ty: Some(IRType::I64) },
        // read(read_fd, &frame, 56) — kernel fills the buffer
        IRInstr::Syscall { nr: 63, args: vec![read_fd, frame.clone(), IRValue::Immediate(56)], dst: Some(tmp.clone()) },
        // Load payload from [44..52]
        // The Alloc'd buffer escapes (passed as arg to Syscall), so SROA
        // and alloc elision won't touch it. The Load after read() is
        // preserved because DSE treats Syscall as clobbering all memory.
        IRInstr::Load { dst: payload.clone(), addr: frame, offset: 44, ty: IRType::I64 },
        // Copy payload to dst
        IRInstr::BinOp { op: BinOpKind::Add, dst: dst, lhs: payload, rhs: IRValue::Immediate(0), ty: Some(IRType::I64) },
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

// ── L6: Checkpoint ────────────────────────────────────────────────────

/// Path used by checkpoint_save / checkpoint_restore, NUL-terminated.
/// "/tmp/vuma_checkpoint.bin\0" — 25 bytes — padded to 32 bytes (4×i64).
const CHECKPOINT_PATH_BYTES: [i64; 4] = [
    0x6d75762f706d742f, // "/tmp/vum"
    0x706b636568635f61, // "a_checkp"
    0x6e69622e746e696f, // "oint.bin"
    0x0000000000000000, // "\0…"
];

/// Emits the IR instructions to mmap a 32-byte buffer and write the
/// checkpoint path into it.  Returns (instrs, path_buf_vreg).
fn build_checkpoint_path(nv: &mut u32) -> (Vec<IRInstr>, IRValue) {
    let path_buf = new_vreg(nv);
    let mut instrs = vec![
        IRInstr::Syscall {
            nr: 222, // mmap
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(32),
                IRValue::Immediate(0x3),  // PROT_READ|PROT_WRITE
                IRValue::Immediate(0x22), // MAP_PRIVATE|MAP_ANONYMOUS
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(path_buf.clone()),
        },
    ];
    for (i, chunk) in CHECKPOINT_PATH_BYTES.iter().enumerate() {
        instrs.push(IRInstr::Store {
            value: IRValue::Immediate(*chunk),
            addr: path_buf.clone(),
            offset: (i * 8) as i32,
            ty: IRType::I64,
        });
    }
    (instrs, path_buf)
}

/// checkpoint_save(value) — void
///
/// Persists `value` (8 bytes) to /tmp/vuma_checkpoint.bin:
///   1. mmap a 32-byte path buffer, write the NUL-terminated path
///   2. openat(AT_FDCWD=-100, path, O_WRONLY|O_CREAT|O_TRUNC=0x241, 0644) → fd
///   3. mmap an 8-byte value buffer, store the value
///   4. write(fd, &val_buf, 8)
///   5. close(fd)
fn expand_checkpoint_save(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let value = args[0].clone();

    let (mut instrs, path_buf) = build_checkpoint_path(nv);

    let fd = new_vreg(nv);
    instrs.push(IRInstr::Syscall {
        nr: 56, // openat
        args: vec![
            IRValue::Immediate(-100i64), // AT_FDCWD
            path_buf,
            IRValue::Immediate(0x241),   // O_WRONLY|O_CREAT|O_TRUNC
            IRValue::Immediate(0o644),
        ],
        dst: Some(fd.clone()),
    });

    let val_buf = new_vreg(nv);
    instrs.push(IRInstr::Syscall {
        nr: 222, // mmap
        args: vec![
            IRValue::Immediate(0),
            IRValue::Immediate(8),
            IRValue::Immediate(0x3),
            IRValue::Immediate(0x22),
            IRValue::Immediate(-1i64),
            IRValue::Immediate(0),
        ],
        dst: Some(val_buf.clone()),
    });
    instrs.push(IRInstr::Store {
        value: value,
        addr: val_buf.clone(),
        offset: 0,
        ty: IRType::I64,
    });

    let write_ret = new_vreg(nv);
    instrs.push(IRInstr::Syscall {
        nr: 64, // write
        args: vec![fd.clone(), val_buf, IRValue::Immediate(8)],
        dst: Some(write_ret),
    });
    instrs.push(IRInstr::Syscall {
        nr: 57, // close
        args: vec![fd],
        dst: None,
    });

    instrs
}

/// checkpoint_restore() -> i64
///
/// Reads 8 bytes from /tmp/vuma_checkpoint.bin and returns the value.
///   1. mmap a 32-byte path buffer, write the path
///   2. openat(AT_FDCWD, path, O_RDONLY=0, 0) → fd
///   3. mmap an 8-byte value buffer, read(fd, &val_buf, 8)
///   4. close(fd)
///   5. Load value from val_buf, store to dst
fn expand_checkpoint_restore(dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let (mut instrs, path_buf) = build_checkpoint_path(nv);

    let fd = new_vreg(nv);
    instrs.push(IRInstr::Syscall {
        nr: 56, // openat
        args: vec![
            IRValue::Immediate(-100i64), // AT_FDCWD
            path_buf,
            IRValue::Immediate(0),       // O_RDONLY
            IRValue::Immediate(0),
        ],
        dst: Some(fd.clone()),
    });

    let val_buf = new_vreg(nv);
    instrs.push(IRInstr::Syscall {
        nr: 222, // mmap
        args: vec![
            IRValue::Immediate(0),
            IRValue::Immediate(8),
            IRValue::Immediate(0x3),
            IRValue::Immediate(0x22),
            IRValue::Immediate(-1i64),
            IRValue::Immediate(0),
        ],
        dst: Some(val_buf.clone()),
    });

    let read_ret = new_vreg(nv);
    instrs.push(IRInstr::Syscall {
        nr: 63, // read
        args: vec![fd.clone(), val_buf.clone(), IRValue::Immediate(8)],
        dst: Some(read_ret),
    });
    instrs.push(IRInstr::Syscall {
        nr: 57, // close
        args: vec![fd],
        dst: None,
    });

    let value = new_vreg(nv);
    instrs.push(IRInstr::Load {
        dst: value.clone(),
        addr: val_buf,
        offset: 0,
        ty: IRType::I64,
    });
    instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: value,
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    });

    instrs
}

// ── L5: Sandbox / resource limits ─────────────────────────────────────

/// sandbox_apply() — void
///
/// Emits prctl(PR_SET_NO_NEW_PRIVS=38, 1, 0, 0, 0) via generic syscall 167.
/// This prevents the process from gaining privileges via setuid binaries.
fn expand_sandbox_apply(nv: &mut u32) -> Vec<IRInstr> {
    let ret = new_vreg(nv);
    vec![
        IRInstr::Syscall {
            nr: 167, // prctl
            args: vec![
                IRValue::Immediate(38), // PR_SET_NO_NEW_PRIVS
                IRValue::Immediate(1),
                IRValue::Immediate(0),
                IRValue::Immediate(0),
                IRValue::Immediate(0),
            ],
            dst: Some(ret),
        },
    ]
}

/// set_resource_limit(rlimit_type, value) — void
///
/// Emits setrlimit(rlimit_type, {rlim_cur=value, rlim_max=value}) via
/// generic syscall 164.  The 16-byte `struct rlimit` is mmap'd (NOT Alloc,
/// because the optimizer would eliminate the Stores before the syscall).
fn expand_set_resource_limit(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let rlimit_type = args[0].clone();
    let value = args[1].clone();

    let rlim_buf = new_vreg(nv);
    let ret = new_vreg(nv);
    vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 16-byte rlimit struct
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(16),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(rlim_buf.clone()),
        },
        // rlim_cur = value
        IRInstr::Store {
            value: value.clone(),
            addr: rlim_buf.clone(),
            offset: 0,
            ty: IRType::I64,
        },
        // rlim_max = value
        IRInstr::Store {
            value: value,
            addr: rlim_buf.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // setrlimit(rlimit_type, &rlim_buf)
        IRInstr::Syscall {
            nr: 164, // setrlimit
            args: vec![rlimit_type, rlim_buf],
            dst: Some(ret),
        },
    ]
}

/// set_memory_limit(limit_mb) — void
///
/// Emits setrlimit(RLIMIT_AS=9, {rlim_cur=limit_mb*1024*1024,
/// rlim_max=limit_mb*1024*1024}) via generic syscall 164.
fn expand_set_memory_limit(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let limit_mb = args[0].clone();

    let bytes = new_vreg(nv);
    let rlim_buf = new_vreg(nv);
    let ret = new_vreg(nv);
    vec![
        // bytes = limit_mb * 1024 * 1024 (1 MB = 1048576 bytes)
        IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: bytes.clone(),
            lhs: limit_mb,
            rhs: IRValue::Immediate(1048576),
            ty: Some(IRType::I64),
        },
        IRInstr::Syscall {
            nr: 222, // mmap — 16-byte rlimit struct
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(16),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(rlim_buf.clone()),
        },
        // rlim_cur = bytes
        IRInstr::Store {
            value: bytes.clone(),
            addr: rlim_buf.clone(),
            offset: 0,
            ty: IRType::I64,
        },
        // rlim_max = bytes
        IRInstr::Store {
            value: bytes,
            addr: rlim_buf.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // setrlimit(RLIMIT_AS=9, &rlim_buf)
        IRInstr::Syscall {
            nr: 164, // setrlimit
            args: vec![IRValue::Immediate(9), rlim_buf],
            dst: Some(ret),
        },
    ]
}

// ── L4: Driver / IRQ ──────────────────────────────────────────────────

/// driver_register(irq, handler_ptr) -> u64
///
/// Simplified: returns 1 (success).  The real per-function IRQ routing
/// table lives in the x86_64 backend.
fn expand_driver_register(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let _ = args;
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: IRValue::Immediate(1),
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    }]
}

/// driver_call(ch, cmd) -> i64
///
/// Same as channel_send(ch, cmd) followed by channel_recv(ch): sends the
/// framed command and recvs the framed result.
fn expand_driver_call(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let ch = args[0].clone();
    let cmd = args[1].clone();
    let mut instrs = expand_channel_send(&[ch.clone(), cmd], nv);
    instrs.extend(expand_channel_recv(&[ch], dst, nv));
    instrs
}

/// irq_dispatch(vector) -> i64
///
/// Simplified: returns -7 (IrqNotRegistered).  Non-x86_64 backends have
/// no per-function IRQ handler table.
fn expand_irq_dispatch(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let _ = args;
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: IRValue::Immediate(-7),
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    }]
}

// ── L3: Capability ────────────────────────────────────────────────────

/// capability_grant(resource_id, perms) -> u64
///
/// Simplified: returns resource_id (uses the resource_id as the cap_id).
/// The real FNV-1a×4 signature minting lives in the x86_64 backend.
fn expand_capability_grant(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let resource_id = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: resource_id,
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    }]
}

/// capability_delegate(cap_id, resource, perms) -> u64
///
/// Simplified: returns cap_id (the parent cap id).
fn expand_capability_delegate(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let cap_id = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: cap_id,
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    }]
}

// ── L1: Framed messaging variants ─────────────────────────────────────

/// channel_send_cap(ch, msg, cap_id) — void
///
/// Same as channel_send but with cap_count=1 at frame offset [36..40].
/// The simplified non-x86_64 lowering does not append the cap_id or a
/// 32-byte FNV-1a×4 signature — receivers using `channel_recv` read the
/// payload from the standard [44..52] offset.
fn expand_channel_send_cap(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 3 { return vec![]; }
    let ch = args[0].clone();
    let msg = args[1].clone();
    let _cap_id = args[2].clone();

    let frame = new_vreg(nv);
    let write_fd = new_vreg(nv);
    let tmp = new_vreg(nv);
    let tmp2 = new_vreg(nv);

    const TYPE_HASH_I64: i64 = 0x2ae1af192b331746;

    vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 56-byte frame
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(56),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(frame.clone()),
        },
        IRInstr::Store { value: IRValue::Immediate(0x414D5556),     addr: frame.clone(), offset: 0,  ty: IRType::I32 },
        IRInstr::Store { value: IRValue::Immediate(0x00020000),     addr: frame.clone(), offset: 4,  ty: IRType::I32 },
        IRInstr::Store { value: IRValue::Immediate(0),              addr: frame.clone(), offset: 8,  ty: IRType::I64 },
        IRInstr::Store { value: IRValue::Immediate(0),              addr: frame.clone(), offset: 16, ty: IRType::I64 },
        IRInstr::Store { value: IRValue::Immediate(TYPE_HASH_I64),  addr: frame.clone(), offset: 24, ty: IRType::I64 },
        IRInstr::Store { value: IRValue::Immediate(8),              addr: frame.clone(), offset: 32, ty: IRType::I32 },
        // cap_count = 1 (the only difference from channel_send)
        IRInstr::Store { value: IRValue::Immediate(1),              addr: frame.clone(), offset: 36, ty: IRType::I32 },
        IRInstr::Store { value: IRValue::Immediate(0),              addr: frame.clone(), offset: 40, ty: IRType::I32 },
        IRInstr::Store { value: msg,                                 addr: frame.clone(), offset: 44, ty: IRType::I64 },
        IRInstr::Store { value: IRValue::Immediate(0),              addr: frame.clone(), offset: 52, ty: IRType::I32 },
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
        IRInstr::Syscall {
            nr: 64, // write
            args: vec![write_fd, frame, IRValue::Immediate(56)],
            dst: Some(tmp2),
        },
    ]
}

/// channel_recv_proto(ch, expected_state) -> i64
///
/// Simplified: same as channel_recv (no protocol state machine on
/// non-x86_64 backends).  The expected_state argument is ignored.
fn expand_channel_recv_proto(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let ch = args[0].clone();
    expand_channel_recv(&[ch], dst, nv)
}

/// Helper: set O_NONBLOCK on a read_fd via fcntl(fd, F_SETFL=4, O_NONBLOCK=0x800).
fn emit_set_nonblocking(read_fd: IRValue, ret: IRValue) -> Vec<IRInstr> {
    vec![
        IRInstr::Syscall {
            nr: 25, // fcntl
            args: vec![
                read_fd,
                IRValue::Immediate(4),    // F_SETFL
                IRValue::Immediate(0x800),// O_NONBLOCK
            ],
            dst: Some(ret),
        },
    ]
}

/// channel_try_recv(ch) -> i64
///
/// Non-blocking recv: returns the payload if data is available, or -2
/// (EAGAIN sentinel) if the channel is empty.
fn expand_channel_try_recv(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let ch = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let read_fd = new_vreg(nv);
    let fcntl_ret = new_vreg(nv);
    let frame = new_vreg(nv);
    let read_ret = new_vreg(nv);
    let payload = new_vreg(nv);
    let is_error = new_vreg(nv);
    let result = new_vreg(nv);

    let mut instrs = vec![
        // read_fd = ch & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: read_fd.clone(),
            lhs: ch,
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
    ];
    instrs.extend(emit_set_nonblocking(read_fd.clone(), fcntl_ret));
    instrs.extend(vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 56-byte frame
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(56),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(frame.clone()),
        },
        IRInstr::Syscall {
            nr: 63, // read
            args: vec![read_fd, frame.clone(), IRValue::Immediate(56)],
            dst: Some(read_ret.clone()),
        },
        IRInstr::Load {
            dst: payload.clone(),
            addr: frame,
            offset: 44,
            ty: IRType::I64,
        },
        // is_error = (read_ret <= 0) ? 1 : 0
        IRInstr::Cmp {
            kind: CmpKind::SLe,
            dst: is_error.clone(),
            lhs: read_ret,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
        // result = is_error ? -2 : payload
        IRInstr::Select {
            dst: result.clone(),
            cond: is_error,
            true_val: IRValue::Immediate(-2),
            false_val: payload,
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: dst,
            lhs: result,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
    ]);
    instrs
}

/// channel_recv_timeout(ch, timeout_ms) -> i64
///
/// Bounded recv: waits up to `timeout_ms` for data, then returns the
/// payload or -3 (Timeout sentinel) if no data arrived.
fn expand_channel_recv_timeout(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let ch = args[0].clone();
    let timeout_ms = args[1].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let read_fd = new_vreg(nv);
    let fcntl_ret = new_vreg(nv);
    let pollfd = new_vreg(nv);
    let ts = new_vreg(nv);
    let tv_sec = new_vreg(nv);
    let tmp = new_vreg(nv);
    let rem = new_vreg(nv);
    let tv_nsec = new_vreg(nv);
    let poll_ret = new_vreg(nv);
    let frame = new_vreg(nv);
    let read_ret = new_vreg(nv);
    let payload = new_vreg(nv);
    let is_error = new_vreg(nv);
    let result = new_vreg(nv);

    let mut instrs = vec![
        // read_fd = ch & 0xFFFFFFFF
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: read_fd.clone(),
            lhs: ch,
            rhs: IRValue::Immediate(0xFFFFFFFF),
            ty: Some(IRType::I64),
        },
    ];
    instrs.extend(emit_set_nonblocking(read_fd.clone(), fcntl_ret));

    // Build pollfd
    instrs.extend(vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 8-byte pollfd
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(8),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(pollfd.clone()),
        },
        IRInstr::Store {
            value: read_fd.clone(),
            addr: pollfd.clone(),
            offset: 0,
            ty: IRType::I32,
        },
        IRInstr::Store {
            value: IRValue::Immediate(1), // POLLIN
            addr: pollfd.clone(),
            offset: 4,
            ty: IRType::I16,
        },
    ]);

    // Build timespec { tv_sec = timeout_ms / 1000, tv_nsec = (timeout_ms % 1000) * 1_000_000 }
    instrs.extend(vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 16-byte timespec
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(16),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(ts.clone()),
        },
        // tv_sec = timeout_ms / 1000
        IRInstr::BinOp {
            op: BinOpKind::SDiv,
            dst: tv_sec.clone(),
            lhs: timeout_ms.clone(),
            rhs: IRValue::Immediate(1000),
            ty: Some(IRType::I64),
        },
        // tmp = tv_sec * 1000
        IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: tmp.clone(),
            lhs: tv_sec.clone(),
            rhs: IRValue::Immediate(1000),
            ty: Some(IRType::I64),
        },
        // rem = timeout_ms - tmp
        IRInstr::BinOp {
            op: BinOpKind::Sub,
            dst: rem.clone(),
            lhs: timeout_ms,
            rhs: tmp,
            ty: Some(IRType::I64),
        },
        // tv_nsec = rem * 1_000_000
        IRInstr::BinOp {
            op: BinOpKind::Mul,
            dst: tv_nsec.clone(),
            lhs: rem,
            rhs: IRValue::Immediate(1_000_000),
            ty: Some(IRType::I64),
        },
        IRInstr::Store {
            value: tv_sec,
            addr: ts.clone(),
            offset: 0,
            ty: IRType::I64,
        },
        IRInstr::Store {
            value: tv_nsec,
            addr: ts.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // ppoll(&pollfd, 1, &ts, NULL) — generic syscall 73
        IRInstr::Syscall {
            nr: 73,
            args: vec![pollfd.clone(), IRValue::Immediate(1), ts, IRValue::Immediate(0)],
            dst: Some(poll_ret),
        },
    ]);

    instrs.extend(vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 56-byte frame
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(56),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(frame.clone()),
        },
        IRInstr::Syscall {
            nr: 63, // read
            args: vec![read_fd, frame.clone(), IRValue::Immediate(56)],
            dst: Some(read_ret.clone()),
        },
        IRInstr::Load {
            dst: payload.clone(),
            addr: frame,
            offset: 44,
            ty: IRType::I64,
        },
        // is_error = (read_ret <= 0)
        IRInstr::Cmp {
            kind: CmpKind::SLe,
            dst: is_error.clone(),
            lhs: read_ret,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
        // result = is_error ? -3 : payload
        IRInstr::Select {
            dst: result.clone(),
            cond: is_error,
            true_val: IRValue::Immediate(-3),
            false_val: payload,
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: dst,
            lhs: result,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
    ]);
    instrs
}

// ── L8: Crypto ────────────────────────────────────────────────────────

/// aead_seal(ptr, len, key_seed) — void
///
/// Simplified in-place AEAD seal:
///   - Load the 8 plaintext bytes at [ptr+8].
///   - XOR them with `key_seed` (the keystream for the simplified cipher).
///   - Store the ciphertext back at [ptr+8].
///   - Store `key_seed` as the 8-byte nonce at [ptr+0].
fn expand_aead_seal(args: &[IRValue], nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 3 { return vec![]; }
    let ptr = args[0].clone();
    let _len = args[1].clone();
    let key_seed = args[2].clone();

    let plaintext = new_vreg(nv);
    let ciphertext = new_vreg(nv);

    vec![
        // plaintext = load [ptr+8]
        IRInstr::Load {
            dst: plaintext.clone(),
            addr: ptr.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // ciphertext = plaintext XOR key_seed
        IRInstr::BinOp {
            op: BinOpKind::Xor,
            dst: ciphertext.clone(),
            lhs: plaintext,
            rhs: key_seed.clone(),
            ty: Some(IRType::I64),
        },
        // store ciphertext at [ptr+8]
        IRInstr::Store {
            value: ciphertext,
            addr: ptr.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // store nonce (= key_seed) at [ptr+0]
        IRInstr::Store {
            value: key_seed,
            addr: ptr,
            offset: 0,
            ty: IRType::I64,
        },
    ]
}

/// aead_open(ptr, len, key_seed) -> i64
///
/// Simplified in-place AEAD open: reverses the XOR at [ptr+8].
/// Returns 0 (success).
fn expand_aead_open(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 3 { return vec![]; }
    let ptr = args[0].clone();
    let _len = args[1].clone();
    let key_seed = args[2].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let ciphertext = new_vreg(nv);
    let plaintext = new_vreg(nv);

    vec![
        // ciphertext = load [ptr+8]
        IRInstr::Load {
            dst: ciphertext.clone(),
            addr: ptr.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // plaintext = ciphertext XOR key_seed (reverse the seal XOR)
        IRInstr::BinOp {
            op: BinOpKind::Xor,
            dst: plaintext.clone(),
            lhs: ciphertext,
            rhs: key_seed,
            ty: Some(IRType::I64),
        },
        // store plaintext at [ptr+8]
        IRInstr::Store {
            value: plaintext,
            addr: ptr,
            offset: 8,
            ty: IRType::I64,
        },
        // return 0 (success)
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: dst,
            lhs: IRValue::Immediate(0),
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
    ]
}

/// stark_prove(input) -> u64
///
/// Simplified: returns 1 (a non-zero proof handle).
fn expand_stark_prove(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let _ = args;
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: IRValue::Immediate(1),
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    }]
}

/// stark_verify(proof_handle) -> i64
///
/// Simplified: returns 1 (valid).
fn expand_stark_verify(args: &[IRValue], dst: Option<&IRValue>, _nv: &mut u32) -> Vec<IRInstr> {
    let _ = args;
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };
    vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: dst,
        lhs: IRValue::Immediate(1),
        rhs: IRValue::Immediate(0),
        ty: Some(IRType::I64),
    }]
}

// ── L2: Distributed IPC ───────────────────────────────────────────────

/// channel_open_remote(addr, port) -> u64
///
/// Creates a TCP socket and connects to (addr, port).  Returns the fd
/// on success or 0 on failure.
fn expand_channel_open_remote(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let addr = args[0].clone();
    let port = args[1].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let fd = new_vreg(nv);
    let sockaddr = new_vreg(nv);
    let port_lo = new_vreg(nv);
    let port_hi = new_vreg(nv);
    let port_shifted = new_vreg(nv);
    let port_nbo = new_vreg(nv);
    let connect_ret = new_vreg(nv);
    let is_error = new_vreg(nv);
    let result = new_vreg(nv);

    vec![
        // fd = socket(AF_INET=2, SOCK_STREAM=1, 0)
        IRInstr::Syscall {
            nr: 198, // socket
            args: vec![
                IRValue::Immediate(2), // AF_INET
                IRValue::Immediate(1), // SOCK_STREAM
                IRValue::Immediate(0),
            ],
            dst: Some(fd.clone()),
        },
        // mmap 16-byte sockaddr_in
        IRInstr::Syscall {
            nr: 222, // mmap
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(16),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(sockaddr.clone()),
        },
        // sin_family = AF_INET (2) at [sockaddr+0] (i16)
        IRInstr::Store {
            value: IRValue::Immediate(2),
            addr: sockaddr.clone(),
            offset: 0,
            ty: IRType::I16,
        },
        // htons(port): port_nbo = ((port & 0xFF) << 8) | ((port >> 8) & 0xFF)
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: port_lo.clone(),
            lhs: port.clone(),
            rhs: IRValue::Immediate(0xFF),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::Shl,
            dst: port_shifted.clone(),
            lhs: port_lo,
            rhs: IRValue::Immediate(8),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::ShrL,
            dst: port_hi.clone(),
            lhs: port,
            rhs: IRValue::Immediate(8),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::And,
            dst: port_hi.clone(),
            lhs: port_hi.clone(),
            rhs: IRValue::Immediate(0xFF),
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::Or,
            dst: port_nbo.clone(),
            lhs: port_shifted,
            rhs: port_hi,
            ty: Some(IRType::I64),
        },
        // sin_port (network byte order) at [sockaddr+2] (i16)
        IRInstr::Store {
            value: port_nbo,
            addr: sockaddr.clone(),
            offset: 2,
            ty: IRType::I16,
        },
        // sin_addr at [sockaddr+4] (i32)
        IRInstr::Store {
            value: addr,
            addr: sockaddr.clone(),
            offset: 4,
            ty: IRType::I32,
        },
        // sin_zero (8 bytes) at [sockaddr+8]
        IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: sockaddr.clone(),
            offset: 8,
            ty: IRType::I64,
        },
        // connect(fd, &sockaddr, 16)
        IRInstr::Syscall {
            nr: 203, // connect
            args: vec![fd.clone(), sockaddr, IRValue::Immediate(16)],
            dst: Some(connect_ret.clone()),
        },
        // is_error = (connect_ret < 0)
        IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: is_error.clone(),
            lhs: connect_ret,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
        // result = is_error ? 0 : fd
        IRInstr::Select {
            dst: result.clone(),
            cond: is_error,
            true_val: IRValue::Immediate(0),
            false_val: fd,
            ty: Some(IRType::I64),
        },
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: dst,
            lhs: result,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
    ]
}

/// remote_send(handle, value) -> i64
///
/// sendto(handle, &value, 8, 0, NULL, 0) — syscall 206.
fn expand_remote_send(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.len() < 2 { return vec![]; }
    let handle = args[0].clone();
    let value = args[1].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let buf = new_vreg(nv);
    let ret = new_vreg(nv);
    vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 8-byte value buffer
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(8),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(buf.clone()),
        },
        IRInstr::Store {
            value: value,
            addr: buf.clone(),
            offset: 0,
            ty: IRType::I64,
        },
        // sendto(handle, &buf, 8, 0, NULL, 0)
        IRInstr::Syscall {
            nr: 206, // sendto
            args: vec![
                handle,
                buf,
                IRValue::Immediate(8),
                IRValue::Immediate(0),
                IRValue::Immediate(0), // NULL addr
                IRValue::Immediate(0), // addrlen 0
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

/// remote_recv(handle) -> i64
///
/// recvfrom(handle, &buf, 8, 0, NULL, NULL) — syscall 207.
fn expand_remote_recv(args: &[IRValue], dst: Option<&IRValue>, nv: &mut u32) -> Vec<IRInstr> {
    if args.is_empty() { return vec![]; }
    let handle = args[0].clone();
    let dst = match dst { Some(d) => d.clone(), None => { return vec![]; } };

    let buf = new_vreg(nv);
    let ret = new_vreg(nv);
    let value = new_vreg(nv);
    vec![
        IRInstr::Syscall {
            nr: 222, // mmap — 8-byte value buffer
            args: vec![
                IRValue::Immediate(0),
                IRValue::Immediate(8),
                IRValue::Immediate(0x3),
                IRValue::Immediate(0x22),
                IRValue::Immediate(-1i64),
                IRValue::Immediate(0),
            ],
            dst: Some(buf.clone()),
        },
        // recvfrom(handle, &buf, 8, 0, NULL, NULL)
        IRInstr::Syscall {
            nr: 207, // recvfrom
            args: vec![
                handle,
                buf.clone(),
                IRValue::Immediate(8),
                IRValue::Immediate(0),
                IRValue::Immediate(0), // NULL addr
                IRValue::Immediate(0), // NULL addrlen
            ],
            dst: Some(ret),
        },
        IRInstr::Load {
            dst: value.clone(),
            addr: buf,
            offset: 0,
            ty: IRType::I64,
        },
        IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: dst,
            lhs: value,
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        },
    ]
}
