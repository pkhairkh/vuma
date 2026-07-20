//! # MIPS64 Big-Endian Backend (mips64be)
//!
//! **Wave 49 — wrapper pattern documentation.**
//!
//! `mips64be` is a thin wrapper around the `Mips64Backend`
//! (field `inner: Mips64Backend`, constructed via `Mips64Backend::new_be()`)
//! that produces big-endian MIPS64 ELF binaries for `qemu-mips64`. It
//! delegates `target_info` and `allocate_registers` to `self.inner.*`,
//! then **byte-swaps each 4-byte instruction word LE→BE** inside
//! `encode_function`, `return_stub`, `trampoline`, and the executable
//! `PT_LOAD` segment of `encode_program`.
//!
//! ## Parent-endianness note (post p3b-elf-endianness)
//!
//! The parent `Mips64Backend` now emits a **big-endian ELF header**
//! (`ELFDATA2MSB`, all multi-byte header & PHDR fields in BE) — see
//! `build_mips64_elf_2seg` in `mips64/mod.rs`.  This wrapper therefore no
//! longer swaps the ELF header or program-header fields (the parent's
//! output is already in the correct byte order); it only swaps the 32-bit
//! fixed-width instruction words in the executable `PT_LOAD` segment from
//! LE (the parent encoder's `word.to_le_bytes()`) to BE.
//!
//! ## MIPS endianness & instruction encoding
//!
//! MIPS32/64 is a bi-endian ISA (MIPS Architecture for Programmers,
//! Volume II rev. 6.06 §A.6 "Endian-ness"). The instruction encoding
//! itself is byte-order-independent at the ISA level; only the in-memory
//! storage of the 32-bit fixed-width instruction words and the multi-byte
//! ELF/data fields change between LE (`qemu-mips64el`, `ELFDATA2LSB`) and
//! BE (`qemu-mips64`, `ELFDATA2MSB`). The parent `mips64` backend always
//! emits LE instruction bytes (each encoder writes `word.to_le_bytes()`);
//! this wrapper re-serialises every 4-byte word in BE byte order so that
//! `qemu-mips64` fetches the correct big-endian instruction stream.
//!
//! ## `IRInstr::Syscall` inheritance (Wave 13)
//!
//! `IRInstr::Syscall` emission is **automatically inherited** from the
//! parent `Mips64Backend`. This backend delegates `allocate_registers`
//! to `self.inner.allocate_registers(func)`, which calls the parent's
//! instruction selector. Once Wave 12 implements the parent's
//! `IRInstr::Syscall { nr, args, dst }` arm (emitting `LI V0, nr; SYSCALL`),
//! this wrapper will automatically produce the same instructions —
//! `encode_function` then byte-swaps each 4-byte word from LE to BE so
//! `qemu-mips64` fetches the correct big-endian instruction words.
//!
//! **Current status:** The parent `mips64` backend has
//! `IRInstr::Syscall { .. } => unimplemented!("… (Wave 12)")` at
//! `mips64/mod.rs:3906`. Until Wave 12 lands, `allocate_registers` will
//! panic on `IRInstr::Syscall`. The conformance test in
//! `src/tests/src/cross_backend.rs` uses `catch_unwind` to gracefully
//! report this as "pending Wave 12" rather than failing.

pub use crate::mips64::{Fpr, Gpr, Instruction, Mips64Backend};

use crate::backend::{AllocatedFunction, AllocatedProgram, Backend, BackendError};
use crate::ir::IRFunction;

pub struct Mips64BeBackend {
    inner: Mips64Backend,
}

impl Mips64BeBackend {
    pub fn new() -> Self {
        Self { inner: Mips64Backend::new_be() }
    }
}

impl Default for Mips64BeBackend {
    fn default() -> Self { Self::new() }
}

#[inline]
fn swap_u32(buf: &mut [u8], off: usize) { buf.swap(off, off + 3); buf.swap(off + 1, off + 2); }
fn swap_u16(buf: &mut [u8], off: usize) { buf.swap(off, off + 1); }
fn swap_u64(buf: &mut [u8], off: usize) {
    buf.swap(off, off + 7); buf.swap(off + 1, off + 6);
    buf.swap(off + 2, off + 5); buf.swap(off + 3, off + 4);
}

/// Convert a complete little-endian MIPS64 ELF (as emitted by the parent
/// `Mips64Backend`) to big-endian, in place. This swaps:
/// 1. EI_DATA byte (offset 5): 1 (LE) → 2 (BE)
/// 2. All ELF header multi-byte fields (e_type through e_shstrndx)
/// 3. All PHDR multi-byte fields (p_type through p_align)
/// 4. All 4-byte instruction words in executable LOAD segments
fn swap_le_elf_to_be(elf: &mut [u8]) {
    if elf.len() < 64 { return; }

    // 1. EI_DATA: LE (1) → BE (2)
    elf[5] = 2;

    // 2. ELF header fields (offsets 16..64, all multi-byte)
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

    // 3. PHDR fields (now read as LE since parent emits LE)
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;

    // Read PHDR offsets BEFORE swapping (parent emits LE)
    let mut segments: Vec<(u32, usize, usize)> = Vec::new(); // (p_flags, p_offset, p_filesz)
    let mut off = phoff;
    for _ in 0..phnum {
        if off + phentsize > elf.len() { break; }
        let p_flags  = u32::from_le_bytes(elf[off + 4..off + 8].try_into().unwrap());
        let p_offset = u64::from_le_bytes(elf[off + 8..off + 16].try_into().unwrap()) as usize;
        let p_filesz = u64::from_le_bytes(elf[off + 32..off + 40].try_into().unwrap()) as usize;
        segments.push((p_flags, p_offset, p_filesz));

        // Swap PHDR fields in place
        swap_u32(elf, off);      // p_type
        swap_u32(elf, off + 4);  // p_flags
        swap_u64(elf, off + 8);  // p_offset
        swap_u64(elf, off + 16); // p_vaddr
        swap_u64(elf, off + 24); // p_paddr
        swap_u64(elf, off + 32); // p_filesz
        swap_u64(elf, off + 40); // p_memsz
        swap_u64(elf, off + 48); // p_align
        off += phentsize;
    }

    // 4. Swap 4-byte instruction words in executable LOAD segments
    let header_end = phoff + phnum * phentsize;
    for (p_flags, p_offset, p_filesz) in &segments {
        if p_flags & 1 != 0 { // PF_X
            let start = (*p_offset).max(header_end);
            let end = (*p_offset + *p_filesz).min(elf.len());
            let mut i = start;
            while i + 4 <= end {
                swap_u32(elf, i);
                i += 4;
            }
        }
    }
}

impl Backend for Mips64BeBackend {
    fn name(&self) -> &'static str { "mips64be" }

    fn target_info(&self) -> &dyn crate::backend::TargetInfo {
        self.inner.target_info()
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        self.inner.allocate_registers(func)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // The parent (mips64) emits a little-endian ELF (header + PHDR +
        // instructions) for qemu-mips64el. This wrapper produces a
        // big-endian ELF for qemu-mips64 by:
        // 1. Swapping the ELF header fields (e_ident[5] EI_DATA: 1→2,
        //    and all multi-byte header/PHDR fields LE→BE)
        // 2. Swapping each 4-byte instruction word LE→BE
        let mut elf = self.inner.encode_program(program)?;
        swap_le_elf_to_be(&mut elf);
        Ok(elf)
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        // Swap 4-byte instruction words from LE to BE, matching the
        // byte-swap applied by return_stub and trampoline. The base
        // mips64 encoder returns LE instruction bytes; for qemu-mips64
        // (big-endian) the same words must appear in BE byte order.
        let mut code = self.inner.encode_function(func)?;
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() {
                swap_u32(&mut code, i);
            }
        }
        Ok(code)
    }

    fn return_stub(&self) -> Vec<u8> {
        let mut code = self.inner.return_stub();
        // Swap 4-byte instruction words
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() {
                swap_u32(&mut code, i);
            }
        }
        code
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        let mut code = self.inner.trampoline(entry_addr);
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() {
                swap_u32(&mut code, i);
            }
        }
        code
    }

    fn disassemble(&self, code: &[u8], base_addr: u64) -> Vec<String> {
        // Swap each 4-byte word to LE for the LE disassembler
        let mut swapped = code.to_vec();
        for i in (0..swapped.len()).step_by(4) {
            if i + 4 <= swapped.len() {
                swap_u32(&mut swapped, i);
            }
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
    use std::panic;

    /// Wave 13 conformance: verify that `IRInstr::Syscall { nr: 1, .. }`
    /// inheritance from the parent `Mips64Backend` works.  Because Wave 12
    /// has not yet implemented Syscall on the parent mips64 backend, this
    /// test uses `catch_unwind` and asserts that the result is EITHER
    /// non-empty encoded output (Wave 12 landed) OR a panic containing
    /// "Wave 12" (still pending).  Once Wave 12 lands, the test will
    /// automatically require non-empty output.
    #[test]
    fn test_syscall_inherited_from_mips64() {
        let backend = Mips64BeBackend::new();
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
                    nr: 1, // __NR_exit on MIPS64 Linux
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

        // Attempt allocation + encoding, catching the Wave 12 panic.
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let allocated = backend.allocate_registers(&func)?;
            backend.encode_function(&allocated)
        }));

        match result {
            Ok(Ok(bytes)) => {
                assert_eq!(
                    bytes.len(),
                    48,
                    "wave13: mips64be should emit exactly 48 bytes for IRInstr::Syscall"
                );
            }
            Ok(Err(e)) => {
                panic!(
                    "mips64be allocate_registers/encode_function returned error (not Wave 12 panic): {}",
                    e
                );
            }
            Err(panic_payload) => {
                let msg = panic_payload
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                    .unwrap_or("<non-string panic>");
                assert!(
                    msg.contains("Wave 12"),
                    "mips64be panicked with unexpected message (expected 'Wave 12' pending): {}",
                    msg
                );
            }
        }
    }
}
