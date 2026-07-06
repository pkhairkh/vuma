//! # ARM32 Big-Endian Backend (armeb)
//!
//! Thin wrapper around the little-endian `arm32` backend that produces
//! big-endian ARM32 ELF binaries. Swaps all 4-byte instruction words
//! and ELF header fields from LE to BE.

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
#[inline]
fn swap_u64(buf: &mut [u8], off: usize) {
    buf.swap(off, off + 7); buf.swap(off + 1, off + 6);
    buf.swap(off + 2, off + 5); buf.swap(off + 3, off + 4);
}

fn swap_le_elf32_to_be(elf: &mut Vec<u8>) {
    if elf.len() < 52 { return; } // ELF32 header is 52 bytes

    // Read offsets BEFORE swapping (they're still LE at this point)
    let phoff = u32::from_le_bytes(elf[28..32].try_into().unwrap()) as usize;
    let phnum = u16::from_le_bytes(elf[44..46].try_into().unwrap()) as usize;
    let shoff = u32::from_le_bytes(elf[32..36].try_into().unwrap()) as usize;
    let shnum = u16::from_le_bytes(elf[48..50].try_into().unwrap()) as usize;
    // Read text p_offset from first phdr (still LE)
    let text_offset = if phnum > 0 && phoff + 8 <= elf.len() {
        u32::from_le_bytes(elf[phoff + 4..phoff + 8].try_into().unwrap()) as usize
    } else {
        52 + phnum * 32
    };

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

    // 5. Swap all 4-byte instruction words in text segment
    let mut i = text_offset;
    while i + 4 <= elf.len() {
        swap_u32(elf, i);
        i += 4;
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
        self.inner.encode_function(func)
    }

    fn return_stub(&self) -> Vec<u8> {
        let mut code = self.inner.return_stub();
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() { swap_u32(&mut code, i); }
        }
        code
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        let mut code = self.inner.trampoline(entry_addr);
        for i in (0..code.len()).step_by(4) {
            if i + 4 <= code.len() { swap_u32(&mut code, i); }
        }
        code
    }

    fn disassemble(&self, code: &[u8], base_addr: u64) -> Vec<String> {
        let mut swapped = code.to_vec();
        for i in (0..swapped.len()).step_by(4) {
            if i + 4 <= swapped.len() { swap_u32(&mut swapped, i); }
        }
        self.inner.disassemble(&swapped, base_addr)
    }
}
