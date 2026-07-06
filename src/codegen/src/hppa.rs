//! # HPPA (HP PA-RISC) Backend — Stub
//!
//! Currently a stub that produces valid PA-RISC ELF binaries.
//! Full instruction encoding is not yet implemented.

use crate::backend::{AllocatedFunction, AllocatedProgram, Backend, BackendError, TargetInfo, Endianness};
use crate::ir::{IRType, IRFunction};

pub struct HppaBackend;
impl HppaBackend { pub fn new() -> Self { Self } }
impl Default for HppaBackend { fn default() -> Self { Self::new() } }

pub struct HppaTargetInfo;
impl TargetInfo for HppaTargetInfo {
    fn isa_name(&self) -> &'static str { "hppa" }
    fn target_triple(&self) -> &'static str { "hppa-unknown-linux-gnu" }
    fn elf_machine_type(&self) -> u16 { 15 }
    fn default_base_address(&self) -> u64 { 0x10000 }
    fn pointer_width(&self) -> usize { 32 }
    fn size_of(&self, ty: &IRType) -> usize { match ty { IRType::I8|IRType::U8 => 1, IRType::I16|IRType::U16 => 2, IRType::I32|IRType::U32|IRType::F32 => 4, _ => 8 } }
    fn alignment_of(&self, ty: &IRType) -> usize { self.size_of(ty) }
    fn endianness(&self) -> Endianness { Endianness::Big }
    fn has_registers(&self) -> bool { true }
    fn num_gp_regs(&self) -> usize { 32 }
    fn num_simd_fp_regs(&self) -> usize { 0 }
    fn has_hardwired_zero(&self) -> bool { true }
    fn has_link_register(&self) -> bool { true }
    fn has_branch_delay_slots(&self) -> bool { false }
    fn has_toc_pointer(&self) -> bool { false }
    fn has_condition_registers(&self) -> bool { false }
    fn calling_convention_name(&self) -> &'static str { "hppa-cdecl" }
    fn num_int_arg_regs(&self) -> usize { 4 }
    fn num_fp_arg_regs(&self) -> usize { 0 }
    fn stack_alignment(&self) -> usize { 64 }
    fn instruction_alignment(&self) -> usize { 4 }
    fn instruction_width_range(&self) -> (usize, usize) { (4, 4) }
    fn output_format(&self) -> crate::backend::OutputFormat { crate::backend::OutputFormat::Elf32 }
}

impl Backend for HppaBackend {
    fn name(&self) -> &'static str { "hppa" }
    fn target_info(&self) -> &dyn TargetInfo { &HppaTargetInfo }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        Ok(AllocatedFunction {
            name: func.name.clone(),
            blocks: vec![crate::backend::AllocatedBlock {
                label: "entry".to_string(),
                instructions: vec![],
                code_offset: 0,
            }],
            frame_size: 0,
            callee_saved: vec![],
            spill_slots: 0,
            code_size: 0,
            wasm_func_type: None,
            wasm_locals: None,
            relocations: vec![],
            
        })
    }

    fn encode_function(&self, _func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        Ok(Vec::new())
    }

    fn encode_program(&self, _program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // Build minimal HPPA ELF with just a _start stub
        let code: Vec<u8> = vec![
            0x08, 0x00, 0x02, 0x40, // NOP (LDIL r0, 0)
            0x08, 0x00, 0x02, 0x40, // NOP
            0xE4, 0x00, 0x10, 0x00, // GATE (ble 0x100(%sr2,%r0)) — syscall exit
        ];

        let base_addr: u32 = 0x10000;
        let text_offset: u32 = 52 + 3 * 32; // ELF32 header + 3 phdrs
        let entry = base_addr + text_offset;

        let mut elf = Vec::new();
        // e_ident
        elf.extend_from_slice(&[0x7f, b'E', b'L', b'F', 1, 2, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
        // ELF32 header (BE)
        elf.extend_from_slice(&2u16.to_be_bytes()); // e_type
        elf.extend_from_slice(&15u16.to_be_bytes()); // e_machine = EM_PARISC
        elf.extend_from_slice(&1u32.to_be_bytes()); // e_version
        elf.extend_from_slice(&entry.to_be_bytes()); // e_entry
        elf.extend_from_slice(&52u32.to_be_bytes()); // e_phoff
        elf.extend_from_slice(&0u32.to_be_bytes()); // e_shoff
        elf.extend_from_slice(&0u32.to_be_bytes()); // e_flags
        elf.extend_from_slice(&52u16.to_be_bytes()); // e_ehsize
        elf.extend_from_slice(&32u16.to_be_bytes()); // e_phentsize
        elf.extend_from_slice(&3u16.to_be_bytes()); // e_phnum
        elf.extend_from_slice(&40u16.to_be_bytes()); // e_shentsize
        elf.extend_from_slice(&0u16.to_be_bytes()); // e_shnum
        elf.extend_from_slice(&0u16.to_be_bytes()); // e_shstrndx

        // Phdr 1: LOAD (text, RX) — include ELF header in segment so
        // p_offset (0) ≡ p_vaddr (0x10000) mod p_align (0x1000). This is
        // required by qemu's ELF loader (and the ELF spec).
        let text_filesz = text_offset + code.len() as u32;
        elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset = 0 (include header)
        elf.extend_from_slice(&base_addr.to_be_bytes()); // p_vaddr
        elf.extend_from_slice(&base_addr.to_be_bytes()); // p_paddr
        elf.extend_from_slice(&text_filesz.to_be_bytes()); // p_filesz (header + code)
        elf.extend_from_slice(&text_filesz.to_be_bytes()); // p_memsz
        elf.extend_from_slice(&5u32.to_be_bytes()); // p_flags = PF_R | PF_X
        elf.extend_from_slice(&0x1000u32.to_be_bytes()); // p_align
        // Phdr 2: LOAD (data, RW)
        elf.extend_from_slice(&1u32.to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&(base_addr + 0x10000).to_be_bytes());
        elf.extend_from_slice(&(base_addr + 0x10000).to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&0x1000u32.to_be_bytes());
        elf.extend_from_slice(&6u32.to_be_bytes());
        elf.extend_from_slice(&0x1000u32.to_be_bytes());
        // Phdr 3: GNU_STACK
        elf.extend_from_slice(&0x6474e551u32.to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&0u32.to_be_bytes());
        elf.extend_from_slice(&6u32.to_be_bytes());
        elf.extend_from_slice(&4u32.to_be_bytes());

        elf.extend_from_slice(&code);
        Ok(elf)
    }

    fn return_stub(&self) -> Vec<u8> { vec![0x08, 0x00, 0x02, 0x40] }
    fn trampoline(&self, _entry_addr: u64) -> Vec<u8> { vec![0x08, 0x00, 0x02, 0x40] }
    fn disassemble(&self, code: &[u8], _base_addr: u64) -> Vec<String> {
        code.chunks(4).map(|c| format!("0x{:08x}", u32::from_be_bytes([c[0],c[1],c[2],c[3]]))).collect()
    }
}
