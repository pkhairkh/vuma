//! # AArch64 Big-Endian Backend (aarch64_be)
//!
//! Thin wrapper around the little-endian `arm64` backend that produces
//! big-endian AArch64 ELF binaries.
//!
//! Key difference from ppc64le/mips64be: AArch64 instructions are ALWAYS
//! little-endian encoded, even on big-endian data systems. So we only
//! swap ELF header fields and program headers — NOT instruction words.

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
