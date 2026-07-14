//! # AArch64 Big-Endian Backend (aarch64_be)
//!
//! **Wave 49 — wrapper pattern documentation.**
//!
//! `aarch64_be` is a thin wrapper around the little-endian `AArch64Backend`
//! (field `inner: AArch64Backend`) that produces big-endian AArch64 ELF
//! binaries. It delegates `target_info`, `allocate_registers`, and
//! `emit_function_regalloc` (i.e. `encode_function`, `return_stub`,
//! `trampoline`, `disassemble`) verbatim to `self.inner.*` — and, crucially,
//! returns the parent's instruction bytes **UNCHANGED**.
//!
//! ## Why no instruction byte-swap?
//!
//! Per the ARM Architecture Reference Manual for AArch64 (ARM DDI 0487,
//! section D6.1.3 "Instruction fetches"), AArch64 instruction fetches are
//! **always little-endian**, regardless of the data endianness selected by
//! `PSTATE.E` (or by the ELF `EI_DATA` byte). The big-endian AArch64 ABI
//! therefore only swaps data loads/stores (LDR/STR with `PSTATE.E=1`); the
//! 32-bit instruction words are stored LE in `.text`. Because the parent
//! `AArch64Backend` already emits LE instruction bytes, `encode_function`
//! simply forwards them — no swap. Only the ELF header / PHDR fields (the
//! data ABI) are flipped in `encode_program` via `swap_le_elf_to_be`.
//!
//! ## `IRInstr::Syscall` inheritance (Wave 13)
//!
//! `IRInstr::Syscall` emission is **automatically inherited** from the
//! parent `AArch64Backend`. This backend delegates `allocate_registers`
//! to `self.inner.allocate_registers(func)`, which calls the parent's
//! instruction selector (`arm64::InstructionSelector::select_from_ir`).
//! The parent's `IRInstr::Syscall { nr, args, dst }` arm (added in Wave 11,
//! `arm64.rs:4538`) emits `MOVZ X8, nr; SVC #0` with arg moves into X0-X5.
//! Because AArch64 instructions are always LE-encoded (even on BE data
//! systems per ARM ARM D6.1.3), `encode_function` returns the parent's
//! bytes as-is — no byte-swap needed. The conformance test in
//! `src/tests/src/cross_backend.rs::test_syscall_conformance_all_backends`
//! verifies that a `Syscall { nr: 1, .. }` produces non-empty encoded
//! output on this backend.

use crate::backend::{AllocatedFunction, AllocatedProgram, Backend, BackendError, AArch64Backend};
use crate::ir::IRFunction;

pub struct AArch64BeBackend {
    inner: AArch64Backend,
}

impl AArch64BeBackend {
    pub fn new() -> Self { Self { inner: AArch64Backend::new() } }
}

impl Default for AArch64BeBackend {
    fn default() -> Self { Self::new() }
}

#[inline]
fn swap_u16(buf: &mut [u8], off: usize) { buf.swap(off, off + 1); }
#[inline]
fn swap_u32(buf: &mut [u8], off: usize) { buf.swap(off, off + 3); buf.swap(off + 1, off + 2); }
#[inline]
fn swap_u64(buf: &mut [u8], off: usize) {
    buf.swap(off, off + 7); buf.swap(off + 1, off + 6);
    buf.swap(off + 2, off + 5); buf.swap(off + 3, off + 4);
}

/// Convert a little-endian AArch64 ELF to big-endian (data only, not instructions).
fn swap_le_elf_to_be(elf: &mut Vec<u8>) {
    if elf.len() < 64 { return; }

    // 0. Read PHDR offsets BEFORE swapping — the bytes are still LE here.
    //    (Reading them after the header-field swaps below would interpret
    //    already-BE bytes as LE, giving huge wrong offsets and skipping
    //    the PHDR swap loop entirely.)
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;

    // 1. EI_DATA: ELFDATA2LSB (1) → ELFDATA2MSB (2)
    elf[5] = 2;

    // 2. ELF64 header fields (offset 16+)
    swap_u16(elf, 16); // e_type
    swap_u16(elf, 18); // e_machine
    swap_u32(elf, 20); // e_version
    swap_u64(elf, 24); // e_entry
    swap_u64(elf, 32); // e_phoff
    swap_u64(elf, 40); // e_shoff
    swap_u32(elf, 48); // e_flags
    swap_u16(elf, 52); // e_ehsize
    swap_u16(elf, 54); // e_phentsize
    swap_u16(elf, 56); // e_phnum
    swap_u16(elf, 58); // e_shentsize
    swap_u16(elf, 60); // e_shnum
    swap_u16(elf, 62); // e_shstrndx

    // 3. Program headers (each 56 bytes at e_phoff)
    for i in 0..phnum {
        let base = phoff + i * phentsize;
        if base + phentsize > elf.len() { break; }
        swap_u32(elf, base);      // p_type
        swap_u32(elf, base + 4);  // p_flags
        swap_u64(elf, base + 8);  // p_offset
        swap_u64(elf, base + 16); // p_vaddr
        swap_u64(elf, base + 24); // p_paddr
        swap_u64(elf, base + 32); // p_filesz
        swap_u64(elf, base + 40); // p_memsz
        swap_u64(elf, base + 48); // p_align
    }

    // 4. DO NOT swap instruction words — AArch64 instructions are always LE
    //    (per ARM Architecture Reference Manual D6.1.3, instruction fetches
    //    are always LE regardless of PSTATE.E data endianness).
}

impl Backend for AArch64BeBackend {
    fn name(&self) -> &'static str { "aarch64_be" }

    fn target_info(&self) -> &dyn crate::backend::TargetInfo {
        self.inner.target_info()
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        self.inner.allocate_registers(func)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        let mut elf = self.inner.encode_program(program)?;
        swap_le_elf_to_be(&mut elf);
        Ok(elf)
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        self.inner.encode_function(func)
    }

    fn return_stub(&self) -> Vec<u8> {
        // Instructions stay LE — no swap needed
        self.inner.return_stub()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // Instructions stay LE — no swap needed
        self.inner.trampoline(entry_addr)
    }

    fn disassemble(&self, code: &[u8], base_addr: u64) -> Vec<String> {
        // Instructions stay LE — no swap needed
        self.inner.disassemble(code, base_addr)
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
    /// inherited from the parent `AArch64Backend` and produces non-empty
    /// encoded instruction bytes.  Because AArch64 instructions are always
    /// LE-encoded (even on BE data systems), the wrapper returns the parent's
    /// bytes as-is — no byte-swap is applied.
    #[test]
    fn test_syscall_inherited_from_aarch64() {
        let backend = AArch64BeBackend::new();
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
                    nr: 1, // __NR_write on AArch64 Linux
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
            44,
            "wave13: aarch64_be should emit exactly 44 bytes for IRInstr::Syscall"
        );
    }
}
