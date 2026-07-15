//! # ARM32 Big-Endian Backend (armeb)
//!
//! **Wave 49 — wrapper pattern documentation.**
//!
//! `armeb` is a thin wrapper around the little-endian `Arm32Backend` (field
//! `inner: Arm32Backend`) that produces big-endian ARM32 ELF binaries for
//! `qemu-armeb`. It delegates `target_info` and `allocate_registers` to
//! `self.inner.*`, then **word-swaps each 4-byte instruction word LE→BE**
//! inside `encode_function`, `return_stub`, `trampoline`, and the executable
//! `PT_LOAD` segment of `encode_program`. The ELF (32-bit) header / PHDR /
//! SHDR fields are also flipped via `swap_le_elf32_to_be`.
//!
//! ## BE32 vs BE8
//!
//! ARMv7 supports two big-endian modes (ARM ARM v7-C, section A3.2 —
//! "Instruction and data endianness"):
//!   * **BE8** (`CPSR.E = 1`, instruction-fetch always LE): each 4-byte
//!     instruction word is stored **little-endian** even on a BE data
//!     system. Used by all `arm*-linux-gnueabihf` BE targets since
//!     ARMv6T2+.
//!   * **BE32** (`CPSR.E = 0`, classic big-endian): each 4-byte
//!     instruction word is stored **big-endian**, matching the data ABI.
//!     Used by legacy `armeb-*` Linux/uClibc targets and what
//!     `qemu-armeb` expects.
//!
//! `armeb` (this backend) targets **BE32** mode: `encode_function` byte-
//! swaps every 4-byte word LE→BE after the parent `Arm32Backend` emits
//! LE words (each encoder writes `word.to_le_bytes()`), so `qemu-armeb`
//! fetches the correct big-endian instruction words.
//!
//! ## `IRInstr::Syscall` inheritance (Wave 13)
//!
//! `IRInstr::Syscall` emission is **automatically inherited** from the
//! parent `Arm32Backend`. This backend delegates `allocate_registers`
//! to `self.inner.allocate_registers(func)`, which calls the parent's
//! instruction selector. The parent's `IRInstr::Syscall { nr, args, dst }`
//! arm (added in Wave 11, `arm32/mod.rs:6835`) emits `MOV R7, nr; SVC #0`
//! with arg moves into R0-R3 (and stack for args 5-6). `encode_function`
//! then byte-swaps each 4-byte instruction word from LE to BE (BE32 mode)
//! so `qemu-armeb` fetches the correct big-endian instruction words. The
//! conformance test in `src/tests/src/cross_backend.rs` verifies that a
//! `Syscall { nr: 1, .. }` produces non-empty encoded output on this
//! backend.

pub use crate::arm32::{Gpr, Instruction, Arm32Backend};

use crate::backend::{AllocatedFunction, AllocatedProgram, Backend, BackendError};
use crate::ir::IRFunction;

pub struct ArmEbBackend {
    inner: Arm32Backend,
}

impl ArmEbBackend {
    pub fn new() -> Self { Self { inner: Arm32Backend::new() } }
}

impl Default for ArmEbBackend {
    fn default() -> Self { Self::new() }
}

#[inline]
fn swap_u16(buf: &mut [u8], off: usize) { buf.swap(off, off + 1); }
#[inline]
fn swap_u32(buf: &mut [u8], off: usize) { buf.swap(off, off + 3); buf.swap(off + 1, off + 2); }

fn swap_le_elf32_to_be(elf: &mut [u8]) {
    if elf.len() < 52 { return; } // ELF32 header is 52 bytes

    // Read offsets BEFORE swapping (they're still LE at this point)
    let phoff = u32::from_le_bytes(elf[28..32].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[44..46].try_into().unwrap()) as usize;
    let shoff = u32::from_le_bytes(elf[32..36].try_into().unwrap()) as usize;
    let shnum = u16::from_le_bytes(elf[48..50].try_into().unwrap()) as usize;

    // 1. EI_DATA: ELFDATA2LSB (1) → ELFDATA2MSB (2)
    elf[5] = 2;

    // 2. ELF32 header fields (offset 16+)
    swap_u16(elf, 16); swap_u16(elf, 18); swap_u32(elf, 20);
    swap_u32(elf, 24); swap_u32(elf, 28); swap_u32(elf, 32);
    swap_u32(elf, 36); swap_u16(elf, 40); swap_u16(elf, 42);
    swap_u16(elf, 44); swap_u16(elf, 46); swap_u16(elf, 48);
    swap_u16(elf, 50);

    // 3. Program headers (each 32 bytes at phoff)
    for i in 0..phnum {
        let base = phoff + i * 32;
        if base + 32 > elf.len() { break; }
        for j in 0..8 {
            swap_u32(elf, base + j * 4);
        }
    }

    // 4. Section headers (each 40 bytes at shoff) — if present
    if shoff > 0 && shnum > 0 {
        for i in 0..shnum {
            let base = shoff + i * 40;
            if base + 40 > elf.len() { break; }
            for j in 0..10 {
                swap_u32(elf, base + j * 4);
            }
        }
    }

    // 5. Swap all 4-byte instruction words in the executable LOAD segment(s).
    //    ARM armeb Linux uses BE32 mode (classic big-endian), where instructions
    //    are stored in big-endian byte order (unlike BE8 where instructions
    //    are LE). qemu-armeb reads instructions as BE u32, so we must swap
    //    every 4-byte instruction word from LE (as produced by the arm32
    //    encoder) to BE.
    //    Skip the ELF + PHDR header bytes (already swapped field-by-field
    //    above); only swap from header_end onward, inside PF_X segments.
    let header_end = phoff + phnum * 32;
    let mut off = phoff;
    for _ in 0..phnum {
        if off + 32 > elf.len() { break; }
        // Read PHDR fields AFTER the per-field swap above (so they're now BE).
        // ELF32 Phdr layout: p_type(0), p_offset(4), p_vaddr(8), p_paddr(12),
        //   p_filesz(16), p_memsz(20), p_flags(24), p_align(28).
        let p_flags  = u32::from_be_bytes(elf[off + 24..off + 28].try_into().unwrap());
        let p_offset = u32::from_be_bytes(elf[off + 4..off + 8].try_into().unwrap()) as usize;
        let p_filesz = u32::from_be_bytes(elf[off + 16..off + 20].try_into().unwrap()) as usize;
        // PF_X = 0x1 — only flip inside executable segments.
        if p_flags & 1 != 0 {
            let start = p_offset.max(header_end);
            let end = (p_offset + p_filesz).min(elf.len());
            let mut i = start;
            while i + 4 <= end {
                swap_u32(elf, i);
                i += 4;
            }
        }
        off += 32;
    }
}

impl Backend for ArmEbBackend {
    fn name(&self) -> &'static str { "armeb" }

    fn target_info(&self) -> &dyn crate::backend::TargetInfo {
        self.inner.target_info()
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        self.inner.allocate_registers(func)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        let mut elf = self.inner.encode_program(program)?;
        swap_le_elf32_to_be(&mut elf);
        Ok(elf)
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        // BE32: swap instruction words from LE to BE, matching the
        // byte-swap applied by return_stub and trampoline. The base
        // arm32 encoder returns LE instruction bytes (each encoder uses
        // word.to_le_bytes()); for qemu-armeb (BE32 mode) the same words
        // must appear in big-endian byte order.
        let mut code = self.inner.encode_function(func)?;
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() { swap_u32(&mut code, i); }
        }
        Ok(code)
    }

    fn return_stub(&self) -> Vec<u8> {
        // BE32: swap instruction words from LE to BE.
        let mut code = self.inner.return_stub();
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() { swap_u32(&mut code, i); }
        }
        code
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // BE32: swap instruction words from LE to BE.
        let mut code = self.inner.trampoline(entry_addr);
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() { swap_u32(&mut code, i); }
        }
        code
    }

    fn disassemble(&self, code: &[u8], base_addr: u64) -> Vec<String> {
        // BE32: swap BE→LE before handing to the LE disassembler.
        let mut swapped = code.to_vec();
        for i in (0..swapped.len()).step_by(4) {
            if i + 4 <= swapped.len() { swap_u32(&mut swapped, i); }
        }
        self.inner.disassemble(&swapped, base_addr)
    }
}

// ===========================================================================
// Tests — IRInstr::Syscall inheritance (Wave 13)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRBlock, IRFunction, IRInstr, IRTerminator, IRValue};
    use std::collections::HashSet;

    /// Wave 13 conformance: verify that `IRInstr::Syscall { nr: 1, .. }` is
    /// inherited from the parent `Arm32Backend` and produces non-empty
    /// encoded instruction bytes.  `encode_function` byte-swaps each 4-byte
    /// word from LE to BE (BE32 mode), but the output remains non-empty.
    #[test]
    fn test_syscall_inherited_from_arm32() {
        let backend = ArmEbBackend::new();
        let func = IRFunction {
            name: "syscall_test".to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![IRBlock {
                label: "entry".to_string(),
                instructions: vec![IRInstr::Syscall {
                    nr: 1, // __NR_exit on ARM32 Linux (EBI)
                    args: vec![],
                    dst: Some(IRValue::Register(0)),
                }],
                terminator: IRTerminator::Return(vec![]),
                predecessors: HashSet::new(),
                successors: HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        };
        let allocated = backend.allocate_registers(&func).expect("allocate_registers");
        let bytes = backend.encode_function(&allocated).expect("encode_function");
        assert_eq!(
            bytes.len(),
            36,
            "wave13: armeb should emit exactly 36 bytes for IRInstr::Syscall"
        );
    }
}
