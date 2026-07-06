//! # PowerPC64 Little-Endian Backend (ppc64le)
//!
//! Thin wrapper around the big-endian `ppc64` backend (`crate::ppc64`) that
//! produces **little-endian** PPC64 ELF binaries.
//!
//! ## Background
//!
//! PPC64 is a bi-endian ISA. The existing `ppc64` backend generates
//! big-endian PPC64 code (`ELFDATA2MSB`, instruction words stored in
//! big-endian byte order). The little-endian variant — ppc64le — is the
//! same Power ISA but uses `ELFDATA2LSB` ELF containers and stores every
//! 4-byte instruction word in little-endian byte order. The ELFv2 ABI
//! (`e_flags = 0x2`) and `EM_PPC64` (21) machine type are identical
//! between the two; only the byte order differs.
//!
//! ## Approach
//!
//! Rather than duplicate the entire ~7,400-line PPC64 backend, this module
//! re-uses `PPC64Backend` for instruction selection, register allocation,
//! and big-endian ELF emission, then performs a post-processing pass on
//! the resulting ELF bytes that:
//!
//! 1. Flips the `EI_DATA` byte (offset 5) from `ELFDATA2MSB` (2) to
//!    `ELFDATA2LSB` (1).
//! 2. Byte-swaps every multi-byte field of the ELF header (`e_type` …
//!    `e_shstrndx`).
//! 3. Byte-swaps every multi-byte field of each program header (one
//!    `Elf64_Phdr` per `e_phnum`).
//! 4. Byte-swaps every multi-byte field of each section header (one
//!    `Elf64_Shdr` per `e_shnum`).
//! 5. Byte-swaps every 4-byte instruction word inside the executable
//!    `.text` section (located by `sh_type == SHT_PROGBITS` and
//!    `sh_flags & SHF_EXECINSTR`).
//!
//! The ASCII section-name string table (`.shstrtab`) and any padding
//! bytes are not touched (they contain no endian-sensitive multi-byte
//! integers). After this pass, the output is a valid ppc64le ELFv2
//! executable that runs under `qemu-ppc64le`.
//!
//! ## Re-exports
//!
//! `Instruction`, `Gpr`, `Fpr`, and `CrField` are re-exported from
//! `crate::ppc64` so external consumers (tests, disasm, etc.) can use the
//! same types whether targeting ppc64 or ppc64le.

// Re-export the PPC64 ISA primitives — they are identical for ppc64le.
pub use crate::ppc64::{CrField, Fpr, Gpr, Instruction};

use crate::backend::{
    AllocatedFunction, AllocatedProgram, Backend, BackendError,
};
use crate::ir::IRFunction;
use crate::ppc64::PPC64Backend;

// ===========================================================================
// PPC64LEBackend
// ===========================================================================

/// PowerPC64 little-endian backend (ELFv2 ABI, `ppc64le`).
///
/// Wraps an inner [`PPC64Backend`] and post-processes its big-endian ELF
/// output into a little-endian ppc64le ELF (see the module docs for the
/// byte-swap rules).
pub struct PPC64LEBackend {
    inner: PPC64Backend,
}

impl PPC64LEBackend {
    /// Create a new ppc64le backend.
    pub fn new() -> Self {
        Self {
            inner: PPC64Backend::new(),
        }
    }
}

impl Default for PPC64LEBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Byte-swap helpers
// ===========================================================================

#[inline]
fn swap_u16(buf: &mut [u8], off: usize) {
    buf.swap(off, off + 1);
}

#[inline]
fn swap_u32(buf: &mut [u8], off: usize) {
    buf.swap(off, off + 3);
    buf.swap(off + 1, off + 2);
}

#[inline]
fn swap_u64(buf: &mut [u8], off: usize) {
    buf.swap(off, off + 7);
    buf.swap(off + 1, off + 6);
    buf.swap(off + 2, off + 5);
    buf.swap(off + 3, off + 4);
}

#[inline]
fn read_u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn read_u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

/// ELF section-header type constants (matches the values used by
/// `build_ppc64_elf_2seg` in `ppc64/mod.rs`).
const SHT_PROGBITS: u32 = 1;
/// `SHF_EXECINSTR` flag bit (`0x4`) — marks executable sections.
const SHF_EXECINSTR: u64 = 0x4;

/// Convert a big-endian PPC64 ELF produced by `PPC64Backend::encode_program`
/// into a little-endian ppc64le ELF, in place.
///
/// See the module-level docs for the exact swap rules. The input `elf` must
/// be a complete ELF64 BE executable as currently emitted by the ppc64
/// backend (ELF header at offset 0, program headers at `e_phoff`, section
/// headers at `e_shoff`).
fn swap_be_elf_to_le(elf: &mut Vec<u8>) {
    if elf.len() < 64 {
        // Not a complete ELF header — nothing sensible to do.
        return;
    }

    // ── 1. EI_DATA byte: ELFDATA2MSB (2) → ELFDATA2LSB (1) ──────────────
    elf[5] = 1; // ELFDATA2LSB

    // ── 2. ELF header fields after the 16-byte e_ident ─────────────────
    // Layout (Elf64_Ehdr from offset 16):
    //   e_type      : u16 @ 16
    //   e_machine   : u16 @ 18
    //   e_version   : u32 @ 20
    //   e_entry     : u64 @ 24
    //   e_phoff     : u64 @ 32
    //   e_shoff     : u64 @ 40
    //   e_flags     : u32 @ 48
    //   e_ehsize    : u16 @ 52
    //   e_phentsize : u16 @ 54
    //   e_phnum     : u16 @ 56
    //   e_shentsize : u16 @ 58
    //   e_shnum     : u16 @ 60
    //   e_shstrndx  : u16 @ 62
    let mut p = 16usize;
    swap_u16(elf, p); p += 2; // e_type
    swap_u16(elf, p); p += 2; // e_machine
    swap_u32(elf, p); p += 4; // e_version
    swap_u64(elf, p); p += 8; // e_entry
    swap_u64(elf, p); p += 8; // e_phoff
    swap_u64(elf, p); p += 8; // e_shoff
    swap_u32(elf, p); p += 4; // e_flags
    swap_u16(elf, p); p += 2; // e_ehsize
    swap_u16(elf, p); p += 2; // e_phentsize
    swap_u16(elf, p); p += 2; // e_phnum
    swap_u16(elf, p); p += 2; // e_shentsize
    swap_u16(elf, p); p += 2; // e_shnum
    swap_u16(elf, p); p += 2; // e_shstrndx
    // p == 64

    // Read phoff/shoff/phnum/shnum/shentsize now in LE for the loops below.
    let phoff = read_u64_le(elf, 32) as usize;
    let shoff = read_u64_le(elf, 40) as usize;
    let phentsize = read_u16_le(elf, 54) as usize;
    let phnum = read_u16_le(elf, 56) as usize;
    let shentsize = read_u16_le(elf, 58) as usize;
    let shnum = read_u16_le(elf, 60) as usize;

    // ── 3. Program headers (Elf64_Phdr, 56 bytes each) ────────────────
    // Layout:
    //   p_type  : u32 @ 0
    //   p_flags : u32 @ 4
    //   p_offset: u64 @ 8
    //   p_vaddr : u64 @ 16
    //   p_paddr : u64 @ 24
    //   p_filesz: u64 @ 32
    //   p_memsz : u64 @ 40
    //   p_align : u64 @ 48
    for i in 0..phnum {
        let base = phoff + i * phentsize;
        if base + 56 > elf.len() {
            break;
        }
        let mut q = base;
        swap_u32(elf, q); q += 4; // p_type
        swap_u32(elf, q); q += 4; // p_flags
        swap_u64(elf, q); q += 8; // p_offset
        swap_u64(elf, q); q += 8; // p_vaddr
        swap_u64(elf, q); q += 8; // p_paddr
        swap_u64(elf, q); q += 8; // p_filesz
        swap_u64(elf, q); q += 8; // p_memsz
        swap_u64(elf, q); q += 8; // p_align
        // q == base + 56
    }

    // ── 4. Section headers (Elf64_Shdr, 64 bytes each) ────────────────
    // Layout:
    //   sh_name      : u32 @ 0
    //   sh_type      : u32 @ 4
    //   sh_flags     : u64 @ 8
    //   sh_addr      : u64 @ 16
    //   sh_offset    : u64 @ 24
    //   sh_size      : u64 @ 32
    //   sh_link      : u32 @ 40
    //   sh_info      : u32 @ 44
    //   sh_addralign : u64 @ 48
    //   sh_entsize   : u64 @ 56
    //
    // While swapping, remember the offset+size of the executable .text
    // section so we can byte-swap its instruction words in step 5.
    let mut text_off: Option<usize> = None;
    let mut text_sz: Option<usize> = None;
    for i in 0..shnum {
        let base = shoff + i * shentsize;
        if base + 64 > elf.len() {
            break;
        }
        let mut q = base;
        swap_u32(elf, q); q += 4; // sh_name
        swap_u32(elf, q); q += 4; // sh_type
        swap_u64(elf, q); q += 8; // sh_flags
        swap_u64(elf, q); q += 8; // sh_addr
        swap_u64(elf, q); q += 8; // sh_offset
        swap_u64(elf, q); q += 8; // sh_size
        swap_u32(elf, q); q += 4; // sh_link
        swap_u32(elf, q); q += 4; // sh_info
        swap_u64(elf, q); q += 8; // sh_addralign
        swap_u64(elf, q); q += 8; // sh_entsize
        // q == base + 64

        // Read sh_type / sh_flags / sh_offset / sh_size (now LE) to identify
        // the executable .text section.
        let sh_type = read_u32_at(elf, base + 4);
        let sh_flags = read_u64_le(elf, base + 8);
        let sh_offset = read_u64_le(elf, base + 24) as usize;
        let sh_size = read_u64_le(elf, base + 32) as usize;
        if sh_type == SHT_PROGBITS && (sh_flags & SHF_EXECINSTR) != 0 {
            text_off = Some(sh_offset);
            text_sz = Some(sh_size);
        }
    }

    // ── 5. Swap every 4-byte instruction word inside .text ────────────
    //
    // PPC64 instructions are 32-bit fixed-width. The ISA encoding itself is
    // identical between ppc64 and ppc64le; only the in-memory/storage byte
    // order differs. Each 4-byte word that the big-endian backend wrote
    // using `to_be_bytes()` must be re-serialized as `to_le_bytes()` for
    // ppc64le. Swapping each 4-byte group in place is equivalent.
    if let (Some(off), Some(sz)) = (text_off, text_sz) {
        let end = off + sz;
        let mut q = off;
        while q + 4 <= end && q + 4 <= elf.len() {
            swap_u32(elf, q);
            q += 4;
        }
    }
}

/// Read a little-endian u32 at `off` (used after the byte-swap has converted
/// the section header to LE).
#[inline]
fn read_u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Byte-swap every 4-byte word in `bytes` (used for `return_stub` and
/// `trampoline`, which return raw instruction bytes from the BE backend).
fn swap_instruction_words(bytes: &mut Vec<u8>) {
    let mut q = 0usize;
    while q + 4 <= bytes.len() {
        swap_u32(bytes, q);
        q += 4;
    }
}

// ===========================================================================
// Backend trait impl
// ===========================================================================

impl Backend for PPC64LEBackend {
    fn target_info(&self) -> &dyn crate::backend::TargetInfo {
        self.inner.target_info()
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        // Register allocation is endianness-independent. The encoded
        // instruction bytes inside each AllocatedInstruction are still
        // big-endian at this point; they are byte-swapped later inside
        // `encode_program` when the .text section is finalized.
        self.inner.allocate_registers(func)
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        // `encode_function` returns raw concatenated instruction bytes.
        // For ppc64le they must be in LE byte order.
        let mut bytes = self.inner.encode_function(func)?;
        swap_instruction_words(&mut bytes);
        Ok(bytes)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // 1. Let the BE backend produce its ELF.
        let mut elf = self.inner.encode_program(program)?;
        // 2. Convert BE ELF → LE ELF in place.
        swap_be_elf_to_le(&mut elf);
        Ok(elf)
    }

    fn return_stub(&self) -> Vec<u8> {
        let mut bytes = self.inner.return_stub();
        swap_instruction_words(&mut bytes);
        bytes
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        let mut bytes = self.inner.trampoline(entry_addr);
        swap_instruction_words(&mut bytes);
        bytes
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // The ppc64 disassembler decodes 4-byte chunks as big-endian. For
        // ppc64le we swap each chunk before delegating so the mnemonics are
        // correct. The displayed `:08x` word value is the LE word.
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset + 4 <= bytes.len() {
            let chunk = &bytes[offset..offset + 4];
            // LE word = what a ppc64le CPU would fetch.
            let word_le = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            // Re-serialize as BE so the ppc64 BE decoder accepts it.
            let be_bytes = word_le.to_be_bytes();
            let mnemonic = match Instruction::decode(&be_bytes) {
                Ok(inst) => format!("{}", inst),
                Err(_) => format!("unknown(word=0x{:08x})", word_le),
            };
            lines.push(format!("{:#010x}:  {:08x}  {}", pc, word_le, mnemonic));
            offset += 4;
            pc += 4;
        }
        if offset < bytes.len() {
            let remaining = &bytes[offset..];
            lines.push(format!("{:#010x}:  {:02x?}", pc, remaining));
        }
        lines
    }

    fn name(&self) -> &'static str {
        "ppc64le"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_name() {
        let backend = PPC64LEBackend::new();
        assert_eq!(backend.name(), "ppc64le");
    }

    #[test]
    fn test_return_stub_is_le_byte_order() {
        // The BE backend produces BLR (bclr 20, 0, 0) as the 4 bytes of
        //   word = (19<<26) | (20<<21) | (16<<1) | 0 = 0x4E800020
        // serialized big-endian as [4E, 80, 00, 20]. The ppc64le backend
        // must serialize the same word little-endian as [20, 00, 80, 4E].
        let backend = PPC64LEBackend::new();
        let stub = backend.return_stub();
        assert_eq!(stub.len(), 4);
        let word_le = u32::from_le_bytes([stub[0], stub[1], stub[2], stub[3]]);
        assert_eq!(word_le, 0x4E800020);
    }

    #[test]
    fn test_swap_instruction_words_round_trip() {
        let mut bytes: Vec<u8> = vec![0x4E, 0x80, 0x00, 0x20]; // BE BLR
        swap_instruction_words(&mut bytes);
        assert_eq!(bytes, vec![0x20, 0x00, 0x80, 0x4E]);
        // Swap again to recover the original.
        swap_instruction_words(&mut bytes);
        assert_eq!(bytes, vec![0x4E, 0x80, 0x00, 0x20]);
    }
}
