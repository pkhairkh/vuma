//! # MIPS64 Big-Endian Backend (mips64be)
//!
//! Thin wrapper around the little-endian `mips64` backend that produces
//! big-endian MIPS64 ELF binaries. The existing mips64 backend encodes
//! instructions in little-endian byte order (for `qemu-mips64el`). This
//! wrapper swaps all 4-byte instruction words and ELF header fields to
//! big-endian for `qemu-mips64`.

pub use crate::mips64::{Fpr, Gpr, Instruction, Mips64Backend};

use crate::backend::{AllocatedFunction, AllocatedProgram, Backend, BackendError};
use crate::ir::IRFunction;

pub struct Mips64BeBackend {
    inner: Mips64Backend,
}

impl Mips64BeBackend {
    pub fn new() -> Self {
        Self { inner: Mips64Backend::new() }
    }
}

impl Default for Mips64BeBackend {
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

/// Convert a little-endian MIPS64 ELF to big-endian, in place.
fn swap_le_elf_to_be(elf: &mut Vec<u8>) {
    if elf.len() < 64 { return; }

    // 0. Read PHDR offsets BEFORE swapping — the bytes are still LE here.
    //    (Reading them after the header-field swaps below would interpret
    //    already-BE bytes as LE, giving huge wrong offsets and skipping
    //    the PHDR swap loop entirely.)
    let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes(elf[54..56].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[56..58].try_into().unwrap()) as usize;
    let header_end = phoff + phnum * phentsize;

    // 1. EI_DATA: ELFDATA2LSB (1) → ELFDATA2MSB (2)
    elf[5] = 2;

    // 2. ELF header fields (offset 16+)
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

    // 4. Swap all 4-byte instruction words in the executable LOAD segment(s).
    //    The first LOAD segment typically has p_offset=0 (includes ELF header
    //    + PHDRs). We must NOT re-flip those header bytes — they're already
    //    in BE order after the per-field swaps above. Start flipping from
    //    header_end onward, and only inside segments with PF_X set.
    let mut off = phoff;
    for _ in 0..phnum {
        if off + phentsize > elf.len() { break; }
        // Read PHDR fields AFTER the per-field swap above (so they're now BE).
        let p_flags  = u32::from_be_bytes(elf[off + 4..off + 8].try_into().unwrap());
        let p_offset = u64::from_be_bytes(elf[off + 8..off + 16].try_into().unwrap()) as usize;
        let p_filesz = u64::from_be_bytes(elf[off + 32..off + 40].try_into().unwrap()) as usize;
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
        off += phentsize;
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
