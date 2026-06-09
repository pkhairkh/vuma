# VUMA Pi 5 Memory Model Specification

**Document ID:** VUMA-SPEC-W1-07  
**Revision:** 1.0  
**Date:** 2026-03-04  
**Author:** Agent W1-07  
**Status:** Draft  

---

## Table of Contents

1. [Pi 5 Virtual Memory Layout](#1-pi-5-virtual-memory-layout)
2. [Pi 5 Physical Memory Map](#2-pi-5-physical-memory-map)
3. [ARM64 Memory Attributes](#3-arm64-memory-attributes)
4. [ARM64 Atomic Operations for VUMA SyncEdges](#4-arm64-atomic-operations-for-vuma-syncedges)
5. [Pi 5 Cache Architecture](#5-pi-5-cache-architecture)

---

## 1. Pi 5 Virtual Memory Layout

### 1.1 ARM64 Virtual Address Space Overview

The Raspberry Pi 5 is powered by the Broadcom BCM2712 SoC featuring a quad-core Arm Cortex-A76 processor implementing the ARMv8.2-A architecture. Under AArch64, the Cortex-A76 supports two virtual address space widths depending on configuration: the standard 48-bit virtual address space yielding 256 TB per translation regime, and the optional Large Virtual Addressing (LVA) extension providing 52-bit virtual addresses for up to 4 PB of addressable space per regime. The LVA feature is controlled via TCR_EL1.T1SZ (or TCR_EL2.T1SZ for EL2): setting T1SZ to 12 enables 52-bit addressing, while the default T1SZ of 16 yields 48-bit addressing. On the Pi 5, the Linux kernel typically runs with the standard 48-bit configuration, though VUMA may exploit LVA in bare-metal deployments to accommodate large sparse capability address spaces.

The canonical 48-bit virtual address space is split into two equal halves by the top-bit selector (bit 63):

| Region | Address Range | Size | Purpose |
|--------|--------------|------|---------|
| User space (TTBR0) | `0x0000000000000000` - `0x0000ffffffffffff` | 256 TB | Application code, heap, stacks, VUMA user Regions |
| Kernel space (TTBR1) | `0xffff000000000000` - `0xffffffffffffffff` | 256 TB | Kernel text, kernel heap, device mappings, VUMA runtime |

Addresses in the range `0x0001000000000000` - `0xfffeffffffffffff` are non-canonical and any access to them triggers a Translation Fault (ESR_EL2.FSC = 0b000001).

### 1.2 VUMA Region Mapping into Virtual Address Space

VUMA's memory model is organized around typed Regions, each representing a contiguous range of virtual addresses with associated capability descriptors (CapD). The following table defines the canonical virtual address layout for VUMA on Pi 5:

| VUMA Region | Virtual Address Range | Size | Attributes |
|-------------|----------------------|------|------------|
| **VUMA_CAP_TABLE** | `0x0000000000000000` - `0x0000000000ffffff` | 16 MB | Normal, WB-WA, RO, Inner Shareable |
| **VUMA_USER_HEAP** | `0x0000000001000000` - `0x0000003fffffffff` | ~4 GB | Normal, WB-WA, RW, Inner Shareable |
| **VUMA_USER_STACK** (per-core) | `0x0000004000000000` + n*0x10000000 | 256 MB/core | Normal, WB-WA, RW, Inner Shareable |
| **VUMA_SEND_RING** | `0x0000008000000000` - `0x0000008000ffffff` | 16 MB | Device, nGnRnE, RW, Inner Shareable |
| **VUMA_PERSIST_HEAP** | `0x0000010000000000` - `0x000001ffffffffff` | 1 TB | Normal, WB-WA, RW, Inner Shareable |
| **VUMA_DEVICE_MMIO** | `0x0000f00000000000` - `0x0000f0ffffffffff` | 1 TB (sparse) | Device, nGnRE, RW, Inner Shareable |
| **VUMA_KERNEL_RUNTIME** | `0xffff000000000000` - `0xffff00000fffffff` | 256 MB | Normal, WB-WA, RW, Inner Shareable |
| **VUMA_KERNEL_CAP_DB** | `0xffff000010000000` - `0xffff00001fffffff` | 256 MB | Normal, WB-WA, RW, Outer Shareable |

The capability table region (`VUMA_CAP_TABLE`) is mapped read-only in user space. Each 64-byte cache line holds one CapD entry, giving a maximum of 262,144 live capabilities. The user heap region grows upward via `mmap`/`brk`-style allocation. The send ring region maps to the BCM2712's mailbox/ring-buffer hardware for inter-core and inter-process message passing. The persist heap is backed by a reserved physical RAM range and uses write-back write-allocate caching to ensure durability semantics at the cache level before flush-to-dram.

### 1.3 Page Table Structure

The Cortex-A76 on the BCM2712 uses ARMv8.2-A's configurable page table granularity. With 4 KB pages (the default for Linux on Pi 5), the translation lookup proceeds through four levels:

| Level | Table Name | Entry Size | Entries per Table | VA Bits Indexed | Address Shift |
|-------|-----------|------------|-------------------|-----------------|---------------|
| 0 | PGD (Page Global Directory) | 8 bytes | 512 | [47:39] | 39 |
| 1 | PUD (Page Upper Directory) | 8 bytes | 512 | [38:30] | 30 |
| 2 | PMD (Page Middle Directory) | 8 bytes | 512 | [29:21] | 21 |
| 3 | PTE (Page Table Entry) | 8 bytes | 512 | [20:12] | 12 |

Each page table entry is a 64-bit descriptor. The format for level 3 (leaf) PTEs is:

```
[63]    NX (Execute-never, EL0)
[62]    PXN (Privileged Execute-never)
[61:60] Reserved (SBZ)
[59]    Reserved for VUMA: CapD present bit
[58:55] Reserved (SBZ)
[54]    Software use (VUMA: dirty bit)
[53:10] Output address [47:12] (4 KB aligned physical frame)
[9]     AF (Access Flag)
[8]     nG (non-Global)
[7:6]   AP[2:1] (Access Permissions)
[5]     NS (Non-Secure)
[4]     AttrIndx[2] (MAIR index bit 2)
[3:2]   AttrIndx[1:0] (MAIR index bits 1:0)
[1]     Block or Page (must be 1 for level 3)
[0]     Valid
```

If the Large Virtual Addressing (LVA) extension is enabled (52-bit VA), a fifth level is inserted: the P5D (Page Level 5 Directory), indexed by VA bits [51:48], with TCR_EL1.T0SZ set to 12. In this configuration, each PGD entry covers 512 TB, and the lookup becomes 5-level. VUMA's bare-metal runtime (`src/pi5`) detects LVA support by reading `ID_AA64MMFR0_EL1.TGran16` and `ID_AA64MMFR0_EL1.PARange` and conditionally enables 5-level paging when the SoC reports PA space of 40 bits or more (which BCM2712 does: it supports up to 40-bit physical addresses, i.e., 1 TB PA).

For 16 KB or 64 KB page granules (configurable via TCR_EL1.TG0), the level count reduces to 3 or 2, but VUMA standardizes on 4 KB pages for compatibility with Linux and the BCM2712 firmware.

### 1.4 Translation Control Register Configuration for VUMA

The following TCR_EL1 fields are critical for VUMA's virtual memory setup on Pi 5:

```c
// TCR_EL1 configuration for VUMA on BCM2712
#define VUMA_TCR_EL1_DEFAULT \
    (0ULL << 37) |  /* TBI0 = 0: top byte ignored disabled (VUMA uses top byte for tags) */ \
    (0ULL << 36) |  /* TBI1 = 0: top byte ignored disabled */ \
    (0ULL << 32) |  /* AS = 0: 8-bit ASID */ \
    (0ULL << 30) |  /* IPS = 0: 32-bit PA (overridden below) */ \
    (2ULL << 28) |  /* TG1 = 2: 4 KB granule for TTBR1 */ \
    (0x3ULL << 24) | /* SH1 = 3: Inner Shareable for TTBR1 */ \
    (0x1ULL << 22) | /* ORGN1 = 1: Outer WB-WA for TTBR1 */ \
    (0x1ULL << 20) | /* IRGN1 = 1: Inner WB-WA for TTBR1 */ \
    (0xEULL << 16) | /* EPD1 = 0, T1SZ = 16: 48-bit VA for TTBR1 */ \
    (0ULL << 14) |  /* TG0 = 0: 4 KB granule for TTBR0 */ \
    (0x3ULL << 12) | /* SH0 = 3: Inner Shareable for TTBR0 */ \
    (0x1ULL << 10) | /* ORGN0 = 1: Outer WB-WA for TTBR0 */ \
    (0x1ULL << 8)  | /* IRGN0 = 1: Inner WB-WA for TTBR0 */ \
    (0x10ULL << 0)   /* T0SZ = 16: 48-bit VA for TTBR0 */
```

---

## 2. Pi 5 Physical Memory Map

### 2.1 BCM2712 Physical Address Space

The BCM2712 SoC implements a 40-bit physical address space (1 TB), though only a fraction is populated on the Pi 5 board. The physical address map follows the Broadcom VideoCore architecture with the ARM peripheral base relocated compared to earlier BCM2835/BCM2711 designs. The following table documents the complete physical memory layout relevant to VUMA:

| Physical Address Range | Size | Description | VUMA Region Mapping |
|-----------------------|------|-------------|---------------------|
| `0x00000000` - `0x1FFFFFFF` | 512 MB | DRAM (4 GB model: extends further) | VUMA_USER_HEAP, VUMA_PERSIST_HEAP |
| `0x00000000` - `0x7FFFFFFF` | 2 GB | DRAM (4 GB model total) | — |
| `0x00000000` - `0xFFFFFFFF` | 4 GB | DRAM (8 GB model: extends to 0x1FFFFFFFF) | — |
| `0x1C000000` - `0x1C00FFFF` | 64 KB | BCM2712 legacy peripheral registers (UART, SPI, I2C, etc.) | VUMA_DEVICE_MMIO |
| `0x1C100000` - `0x1C10FFFF` | 64 KB | BCM2712 DMA registers | VUMA_DEVICE_MMIO |
| `0x1F000000` - `0x1FFFFFFF` | 16 MB | BCM2712 legacy peripheral expansion | VUMA_DEVICE_MMIO |
| `0x7C000000` - `0x7C00FFFF` | 64 KB | BCM2712 peripheral registers (high alias) | VUMA_DEVICE_MMIO |
| `0x7C100000` - `0x7C10FFFF` | 64 KB | BCM2712 DMA registers (high alias) | VUMA_DEVICE_MMIO |
| `0x7D000000` - `0x7DFFFFFF` | 16 MB | BCM2712 system peripheral expansion | VUMA_DEVICE_MMIO |
| `0x7E000000` - `0x7EFFFFFF` | 16 MB | VideoCore MMIO (GPU, HDMI, display) | VUMA_DEVICE_MMIO |
| `0x7F000000` - `0x7FFFFFFF` | 16 MB | VideoCore peripheral alias | VUMA_DEVICE_MMIO |
| `0x100000000` - `0x1000FFFF` | 64 KB | PCIe MMIO config space (ECAM) | VUMA_DEVICE_MMIO |
| `0x100010000` - `0x1FFFFFFFF` | Variable | PCIe MMIO I/O and memory space | VUMA_DEVICE_MMIO |

### 2.2 DRAM Layout for VUMA (4 GB Model)

On the 4 GB Pi 5, the LPDDR4X-4267 memory is mapped starting at physical address `0x00000000`. The Linux kernel's `/proc/iomem` on a typical Pi 5 shows the following layout, which VUMA must respect when reserving persistent regions:

```
0x00000000 - 0x003FFFFF : reserved (VideoCore firmware)
0x00400000 - 0x005FFFFF : reserved (GPU memory, default 64 MB)
0x00600000 - 0x0060FFFF : reserved (ARM stub/bootloader)
0x00610000 - 0x0F7FFFFF : System RAM (kernel, modules, user space)
0x0F800000 - 0x0FFFFFFF : reserved (GPU memory extension)
0x10000000 - 0xF7FFFFFF : System RAM (high memory, user space)
```

VUMA reserves the following physical ranges from the System RAM for its own use, negotiated with the Linux kernel via CMA (Contiguous Memory Allocator) or reserved-memory device tree entries:

| Physical Range | Size | VUMA Use |
|---------------|------|----------|
| `0x0E000000` - `0x0EFFFFFF` | 16 MB | VUMA Capability Table (CapD array, 256K entries x 64B) |
| `0x0F000000` - `0x0F7FFFFF` | 8 MB | VUMA Send Ring Buffer (per-core mailboxes) |
| `0x0F800000` - `0x0FFFFFFF` | 8 MB | VUMA SyncEdge state (atomic metadata) |
| `0x10000000` - `0x17FFFFFF` | 128 MB | VUMA Persist Heap (durable allocation pool) |

For the 8 GB model, the VUMA Persist Heap extends to 512 MB using physical range `0x10000000` - `0x2FFFFFFF`.

### 2.3 BCM2712 Peripheral Register Definitions

The BCM2712 exposes its peripherals through memory-mapped I/O registers. The primary peripheral base for ARM code running on the Pi 5 is at `0x7C000000` (the "high" alias that avoids conflicts with the VideoCore memory map). The following registers are critical for VUMA's device Region type and Send capability implementation:

#### GPIO Registers (Base: `0x7C200000`)

| Offset | Register | Width | Description |
|--------|----------|-------|-------------|
| `0x000` | GPFSEL0 | 32 | GPIO Function Select 0 (pins 0-9) |
| `0x004` | GPFSEL1 | 32 | GPIO Function Select 1 (pins 10-19) |
| `0x008` | GPFSEL2 | 32 | GPIO Function Select 2 (pins 20-29) |
| `0x00C` | GPFSEL3 | 32 | GPIO Function Select 3 (pins 30-39) |
| `0x01C` | GPSET0 | 32 | GPIO Pin Output Set 0 |
| `0x020` | GPSET1 | 32 | GPIO Pin Output Set 1 |
| `0x028` | GPCLR0 | 32 | GPIO Pin Output Clear 0 |
| `0x02C` | GPCLR1 | 32 | GPIO Pin Output Clear 1 |
| `0x034` | GPLEV0 | 32 | GPIO Pin Level 0 |
| `0x038` | GPLEV1 | 32 | GPIO Pin Level 1 |
| `0x040` | GPEDS0 | 32 | GPIO Pin Event Detect Status 0 |
| `0x044` | GPEDS1 | 32 | GPIO Pin Event Detect Status 1 |
| `0x07C` | GPPUD | 32 | GPIO Pin Pull-up/down Control |
| `0x080` | GPPUDCLK0 | 32 | GPIO Pin Pull-up/down Clock 0 |
| `0x084` | GPPUDCLK1 | 32 | GPIO Pin Pull-up/down Clock 1 |

VUMA maps these into the `VUMA_DEVICE_MMIO` virtual region at offset `0x00200000` from the MMIO base, giving a virtual address of `0x0000f00020000000` for GPFSEL0.

#### Mailbox/Doorbell Registers (Base: `0x7C020000`)

The BCM2712 introduces a new doorbell mechanism for inter-core signaling, replacing the older BCM2835-style mailbox:

| Offset | Register | Width | Description |
|--------|----------|-------|-------------|
| `0x000` | MBOX0_READ | 32 | Mailbox 0 read register |
| `0x004` | MBOX0_STATUS | 32 | Mailbox 0 status (bit 31: full, bit 30: empty) |
| `0x018` | MBOX0_CONFIG | 32 | Mailbox 0 configuration (IRQ enable) |
| `0x020` | MBOX1_WRITE | 32 | Mailbox 1 write register |
| `0x024` | MBOX1_STATUS | 32 | Mailbox 1 status |
| `0x100` | DOORBELL_SET | 32 | Doorbell set register (bit-per-core: bits 0-3) |
| `0x104` | DOORBELL_CLR | 32 | Doorbell clear register |
| `0x108` | DOORBELL_STATUS | 32 | Doorbell status register |

VUMA's Send capability maps to the Doorbell registers for cross-core notifications, while the mailbox channels carry the actual message payload pointers.

#### PCIe 2.0 Controller Registers (Base: `0x100000000`)

The BCM2712 includes a PCIe 2.0 x1 controller accessible via the following address ranges:

| Physical Range | Description |
|---------------|-------------|
| `0x100000000` - `0x10000FFF` | PCIe ECAM config space (4 KB, bus 0, dev 0, func 0) |
| `0x100010000` - `0x10001FFFF` | PCIe I/O space (64 KB) |
| `0x100020000` - `0x13FFFFFFF` | PCIe MMIO memory space (up to ~1 GB) |

VUMA maps PCIe MMIO into the `VUMA_DEVICE_MMIO` region for device driver Regions that interact with PCIe peripherals (e.g., NVMe, network cards attached via the Pi 5's FPC connector).

### 2.4 Device Region Mapping for VUMA

VUMA's "device" Region type is mapped to BCM2712 peripheral ranges using Device memory attributes (see Section 3). The translation from VUMA Region descriptor to physical address follows this formula:

```
phys_addr = VUMA_DEVICE_MMIO_offset + bcm2712_peripheral_base
where:
  bcm2712_peripheral_base = 0x7C000000  (for ARM-side access)
  VUMA_DEVICE_MMIO_offset = region_base - 0x0000f00000000000
```

For example, a VUMA device Region at virtual address `0x0000f00020000000` (GPIO) maps to physical `0x7C200000`.

---

## 3. ARM64 Memory Attributes

### 3.1 Normal Memory vs Device Memory

The ARMv8.2-A architecture defines two fundamental memory types that govern how the Cortex-A76 processor interacts with the memory system: Normal memory and Device memory. This distinction is central to VUMA's capability-based access model because it determines whether the CPU may speculate, cache, or reorder accesses to a given Region.

**Normal memory** is used for RAM-backed storage where the processor is free to perform speculative reads, merge writes, cache data in the L1/L2/L3 caches, and reorder accesses subject only to explicit barrier instructions. Normal memory has configurable inner and outer cacheability attributes as well as shareability domains. All VUMA Regions backed by DRAM (Heap, Stack, Persist, CapTable, KernelRuntime) use Normal memory attributes.

**Device memory** is used for memory-mapped I/O where the processor must not speculate, cache, or merge accesses. Device memory has four sub-types that control reordering and gathering behavior:

| Device Type | Mnemonic | Reordering | Gathering | VUMA Use |
|-------------|----------|------------|----------|----------|
| Device-nGnRnE | nGnRnE | No reordering | No gathering | Send ring (strictest ordering) |
| Device-nGnRE | nGnRE | No reordering | Gathering allowed | General MMIO (most device Regions) |
| Device-nGRE | nGRE | Reordering allowed | Gathering allowed | Framebuffer write-combining |
| Device-GRE | GRE | Reordering + gathering | Gathering allowed | Bulk DMA descriptors |

VUMA uses Device-nGnRnE for the Send ring buffer because message ordering must be strictly preserved: a Send capability's payload must be visible to the receiver before the doorbell notification. Device-nGnRE is used for general MMIO (GPIO, PCIe config, mailbox registers) where the ARM architecture permits write gathering for performance but still prevents reordering relative to other nGnRE accesses to the same device.

### 3.2 Inner/Outer Cacheability and Shareability

The Cortex-A76 implements a three-level cache hierarchy (see Section 5). ARM64 defines cacheability attributes independently for the inner and outer cache domains:

| Cacheability | Encoding (MAIR Attr) | Meaning |
|-------------|---------------------|---------|
| WB-WA (Write-Back Write-Allocate) | `0xFF` (Attr[3:0]=0xF, Attr[7:4]=0xF) | Full cache: reads allocate, writes allocate, write-back on eviction |
| WT (Write-Through) | `0xBB` (Attr[3:0]=0xB, Attr[7:4]=0xB) | Writes go through to next level; read allocation only |
| NC (Non-Cacheable) | `0x44` (Attr[3:0]=0x4, Attr[7:4]=0x4) | No caching; all accesses go to main memory |

VUMA configures MAIR_EL1 with the following attribute indices:

```c
// MAIR_EL1 configuration for VUMA
#define VUMA_MAIR_EL1 \
    (0xFFULL << 0)  |  /* Attr0: Normal, Inner/Outer WB-WA   (VUMA_HEAP) */ \
    (0x04ULL << 8)  |  /* Attr1: Normal, Inner/Outer NC      (VUMA_DMA_BUF) */ \
    (0x44ULL << 16) |  /* Attr2: Device-nGnRnE                (VUMA_SEND_RING) */ \
    (0x04ULL << 24) |  /* Attr3: Device-nGnRE                 (VUMA_DEVICE_MMIO) */ \
    (0xBBULL << 32) |  /* Attr4: Normal, Inner/Outer WT       (VUMA_LOG_BUF) */ \
    (0xFFULL << 40) |  /* Attr5: Normal, Inner WB-WA / Outer NC (VUMA_STACK) */ \
    (0x00ULL << 48) |  /* Attr6: Reserved */ \
    (0x00ULL << 56)     /* Attr7: Reserved */
```

**Shareability** determines which agents must see a coherent view of memory:

| Shareability | Encoding | Scope | VUMA Use |
|-------------|----------|-------|----------|
| Non-shareable (OSH) | `0b00` | Single CPU only | Per-core scratch data |
| Outer Shareable (OSH) | `0b10` | All agents in outer domain (CPU + GPU + DMA) | CapDB (shared with VideoCore) |
| Inner Shareable (ISH) | `0b11` | All CPUs in inner domain (4x Cortex-A76) | Heap, Stack, Persist, Send ring, SyncEdge |

VUMA uses Inner Shareable for almost all user-space Regions because the four Cortex-A76 cores constitute the inner shareability domain on BCM2712. Outer Shareable is used only for the kernel capability database (`VUMA_KERNEL_CAP_DB`) which may be accessed by the VideoCore firmware through the shared memory interface.

### 3.3 Mapping VUMA CapD to ARM64 Page Table Attributes

VUMA's capability descriptor (CapD) defines the access rights for a Region. Each CapD contains permission bits that directly map to ARM64 page table entry fields. The following table specifies the complete mapping:

| VUMA Capability | CapD Bit | ARM64 PTE Field | Encoding | Description |
|----------------|----------|----------------|----------|-------------|
| **Read** | `cap_read` (bit 0) | AP[2:1] | `0b01` (when Read-only) | EL0 read-only access. AP[2]=0, AP[1]=1. |
| **Write** | `cap_write` (bit 1) | AP[2:1] | `0b00` (when Read+Write) | EL0 read-write access. AP[2]=0, AP[1]=0. |
| **Execute** | `cap_exec` (bit 2) | UXN / PXN | UXN=0 (allow EL0 exec) | User execute allowed when cap_exec=1. |
| **Persist** | `cap_persist` (bit 3) | AttrIndx[2:0] | `0b000` (WB-WA) | Persist Regions use Normal WB-WA for durability. |
| **Send** | `cap_send` (bit 4) | AttrIndx[2:0] | `0b010` (Device-nGnRnE) | Send Regions use strict Device ordering. |
| **Device** | `cap_device` (bit 5) | AttrIndx[2:0] | `0b011` (Device-nGnRE) | Device Regions use relaxed Device ordering. |

The encoding logic for AP[2:1] from CapD is:

```c
// VUMA CapD to ARM64 AP[2:1] encoding
static inline uint64_t vuma_capd_to_ap(uint64_t capd_permissions) {
    bool cap_read  = capd_permissions & (1 << 0);
    bool cap_write = capd_permissions & (1 << 1);

    if (cap_read && !cap_write) {
        return 0b01;  // AP[2:1] = 01: read-only at EL0
    } else if (cap_read && cap_write) {
        return 0b00;  // AP[2:1] = 00: read-write at EL0
    } else if (!cap_read && cap_write) {
        // Write-only is not directly expressible in ARM64 AP encoding.
        // VUMA maps this to read-write but sets a software bit in PTE[54]
        // to trap and emulate write-only semantics.
        return 0b00;  // read-write, with software write-only flag
    } else {
        return 0b11;  // AP[2:1] = 11: no access at EL0
    }
}
```

The encoding logic for UXN/PXN from CapD:

```c
static inline uint64_t vuma_capd_to_xn(uint64_t capd_permissions) {
    bool cap_exec = capd_permissions & (1 << 2);

    if (!cap_exec) {
        return (1ULL << 63) | (1ULL << 62);  // UXN=1, PXN=1: no execute
    } else {
        return 0;  // UXN=0, PXN=0: execute allowed at EL0 and EL1
    }
}
```

The encoding logic for AttrIndx from CapD:

```c
static inline uint64_t vuma_capd_to_attrindx(uint64_t capd_permissions) {
    bool cap_persist = capd_permissions & (1 << 3);
    bool cap_send    = capd_permissions & (1 << 4);
    bool cap_device  = capd_permissions & (1 << 5);

    if (cap_send) {
        return 0b010;  // Attr2: Device-nGnRnE
    } else if (cap_device) {
        return 0b011;  // Attr3: Device-nGnRE
    } else if (cap_persist) {
        return 0b000;  // Attr0: Normal WB-WA (durable)
    } else {
        return 0b000;  // Attr0: Normal WB-WA (default)
    }
}
```

### 3.4 Access Flag and Dirty Bit Management

VUMA uses the hardware Access Flag (AF, PTE bit 9) and a software-managed dirty bit (PTE bit 54, the architecture's "software use" bit) for page replacement and persistence tracking. The Cortex-A76 supports Hardware Access Flag (HAF) management via TCR_EL1.HA = 1, which automatically sets AF on first access. VUMA enables HAF for performance:

```c
// Enable Hardware Access Flag management
vuma_tcr_el1 |= (1ULL << 42);  // TCR_EL1.HA = 1
```

For dirty bit tracking, VUMA implements a software-managed scheme: page faults on write access to clean pages are caught by the VUMA runtime's fault handler, which sets the software dirty bit (PTE[54]) and upgrades the page from read-only to read-write by modifying AP[1]. This is essential for the Persist capability: on flush, only pages with PTE[54]=1 need to be written back to durable storage.

---

## 4. ARM64 Atomic Operations for VUMA SyncEdges

### 4.1 VUMA SyncEdge Model

VUMA's SyncEdge is the synchronization primitive that enforces ordering constraints between concurrent accesses to shared Regions. Each SyncEdge carries an ordering annotation from VUMA's memory model:

| VUMA Ordering | Semantics | ARM64 Equivalent |
|---------------|-----------|-----------------|
| **Relaxed** | No ordering guarantee | No barrier |
| **Acquire** | Subsequent reads/writes cannot be reordered before this | LDAR / LDAXR |
| **Release** | Prior reads/writes cannot be reordered after this | STLR / STLXR |
| **AcquireRelease** | Both acquire and release semantics | LDAR then STLR, or LDAXR/STLXR pair |
| **HappensBefore** | Establishes a total order between two specific SyncEdges | DMB ISH + DSB ISH |
| **Atomic** | Indivisible read-modify-write, sequentially consistent | CASAL / LDAXR/STLXR loop + DMB ISH |
| **Locked** | Mutual exclusion with system-wide visibility | LDAXR/STLXR loop + DSB ISH + ISB |

### 4.2 ARM64 Exclusive Access Instructions

The Cortex-A76 (ARMv8.2-A) provides two families of atomic operations: the legacy LDXR/STXR exclusive pair and the newer ARMv8.1-A Large System Extension (LSE) atomic instructions.

#### LDXR/STXR (Exclusive Load/Store)

These instructions implement load-link/store-conditional (LL/SC) semantics. The CPU tracks an "exclusive monitor" per shareability domain:

```asm
// VUMA SyncEdge: Atomic increment of a counter at [x0]
vuma_atomic_increment:
    ldaxr   w1, [x0]           // Load with acquire semantics, set exclusive monitor
    add     w1, w1, #1         // Increment
    stlxr   w2, w1, [x0]       // Store with release semantics, check monitor
    cbnz    w2, vuma_atomic_increment  // Retry if monitor was lost
    dmb     ish                 // Full inner-shareable barrier for Atomic ordering
    ret
```

The exclusive monitor is per-inner-shareability-domain on the BCM2712. Since all four Cortex-A76 cores share the same inner domain, cross-core exclusives work without additional configuration. However, the monitor can be cleared by:
- An external agent (DMA, VideoCore) writing to the same cache line
- A context switch that evicts the monitored address from the local cache
- An explicit CLREX instruction

The exclusive monitor granule on Cortex-A76 is 64 bytes (one cache line). If any byte in the same 64-byte line is modified between LDXR and STXR, the exclusive store will fail.

#### LDAXR/STLXR (Acquire/Release Variants)

These are the preferred variants for VUMA because they encode acquire/release semantics directly in the instruction, eliminating the need for separate barrier instructions in many cases:

| Instruction | Size | Acquire | Release | VUMA Ordering |
|------------|------|---------|---------|---------------|
| LDXR | 32/64 bit | No | No | Relaxed |
| LDAXR | 32/64 bit | Yes | No | Acquire |
| STXR | 32/64 bit | No | No | Relaxed |
| STLXR | 32/64 bit | No | Yes | Release |

For 128-bit exclusive accesses (useful for VUMA's 128-bit capability descriptors), the AArch64 architecture provides LDXP/STXP and their acquire/release variants LDAXP/STLXP:

```asm
// VUMA SyncEdge: Atomic 128-bit capability swap
// x0 = pointer to 128-bit CapD, x2:x3 = new value, returns old value in x4:x5
vuma_cap_swap_128:
    ldaxp   x4, x5, [x0]      // Load 128-bit with acquire
    stlxp   w6, x2, x3, [x0]  // Store 128-bit with release
    cbnz    w6, vuma_cap_swap_128
    dmb     ish
    ret
```

**Important caveat**: ARM architecture mandates that the 128-bit exclusive pair (LDXP/STXP) must always check the success code and retry; it is not valid to use LDXP alone as a 128-bit atomic load. VUMA's code generator always emits the retry loop.

### 4.3 LSE Atomic Instructions (ARMv8.1-A)

The Cortex-A76 implements ARMv8.1-A, which includes the Large System Extension (LSE) providing single-instruction atomic operations. These are more efficient than LDXR/STXR loops because they avoid the retry overhead under contention. VUMA detects LSE support via `ID_AA64ISAR0_EL1.Atomic` and uses LSE instructions when available:

```c
// LSE detection
uint64_t isar0 = read_sysreg(ID_AA64ISAR0_EL1);
bool has_lse = ((isar0 >> 20) & 0xF) >= 2;  // Atomic field >= 2 means LSE implemented
```

The key LSE instructions for VUMA SyncEdges:

| Instruction | Operation | VUMA Ordering | Semantics |
|------------|-----------|---------------|-----------|
| CAS (Compare and Swap) | if [Rs] == Rt, [Rs] = Rt2 | Relaxed | No ordering |
| CASA | if [Rs] == Rt, [Rs] = Rt2 | Acquire | Subsequent ops ordered after |
| CASL | if [Rs] == Rt, [Rs] = Rt2 | Release | Prior ops ordered before |
| CASAL | if [Rs] == Rt, [Rs] = Rt2 | AcquireRelease | Full bidirectional ordering |

```asm
// VUMA SyncEdge: Atomic compare-and-swap with AcquireRelease ordering
// x0 = address, x1 = expected value, x2 = new value
// Returns old value in w0; set Z flag if swap succeeded
vuma_cas_acquire_release:
    casal   w1, w2, [x0]       // Compare-and-swap with acquire+release
    cset    w0, eq              // Return 1 if successful, 0 if not
    ret
```

VUMA's code generator emits a LSE fallback path using LDXR/STXR for compatibility with pre-ARMv8.1 cores:

```asm
// VUMA CAS with runtime LSE detection (simplified)
vuma_cas_generic:
    // Check LSE support (cached in system register or global)
    adrp    x3, vuma_has_lse
    ldrb    w3, [x3, :lo12:vuma_has_lse]
    cbnz    w3, .Llse_path

.Lllsc_path:
    ldaxr   w4, [x0]
    cmp     w4, w1
    bne     .Lcas_fail
    stlxr   w5, w2, [x0]
    cbnz    w5, .Lllsc_path
    dmb     ish
    mov     w0, #1
    ret

.Llse_path:
    casal   w1, w2, [x0]
    cset    w0, eq
    ret

.Lcas_fail:
    mov     w0, #0
    ret
```

### 4.4 ARM64 Barrier Mapping for VUMA Orderings

The following table provides the complete mapping from VUMA SyncEdge orderings to ARM64 barrier sequences:

| VUMA Ordering | Before Access | Access Instruction | After Access | Explanation |
|---------------|--------------|-------------------|-------------|-------------|
| **Relaxed** | (none) | LDR/STR | (none) | No ordering. Compiler barrier only (`asm volatile("" ::: "memory")`). |
| **Acquire** | (none) | LDAR / LDAXR | (none) | LDAR prevents subsequent loads/stores from being reordered before it. |
| **Release** | (none) | STLR / STLXR | (none) | STLR prevents prior loads/stores from being reordered after it. |
| **AcquireRelease** | (none) | LDAXR/STLXR pair | (none) | Combined acquire on load, release on store. |
| **HappensBefore** | DMB ISH | LDAXR/STLXR pair | DSB ISH | DMB ensures all prior inner-shareable accesses are visible; DSB waits for completion. |
| **Atomic** | DMB ISH | CASAL or LDAXR/STLXR loop | DMB ISH | Sequentially consistent: full barrier before and after the RMW. |
| **Locked** | DMB ISH | LDAXR/STLXR loop | DSB ISH + ISB | DSB ensures store completes; ISB flushes pipeline so subsequent instructions see new state. |

### 4.5 Barrier Instruction Reference

| Instruction | Full Name | Scope | Effect |
|------------|-----------|-------|--------|
| `DMB ISH` | Data Memory Barrier, Inner Shareable | All inner-shareable agents | Ensures prior memory accesses are ordered before subsequent memory accesses, visible to all inner-shareable observers (all 4 cores). |
| `DMB ISHST` | Data Memory Barrier, Inner Shareable, Store | Store-side only | Ensures prior stores are ordered before subsequent stores. Weaker than full DMB ISH; suitable for Release stores. |
| `DSB ISH` | Data Synchronization Barrier, Inner Shareable | All inner-shareable agents | Waits until all prior memory accesses complete before executing subsequent instructions. Stronger than DMB. |
| `DSB SY` | Data Synchronization Barrier, System | All system agents | Full system-wide completion barrier. Used only for Device-nGnRnE access completion. |
| `ISB` | Instruction Synchronization Barrier | Local CPU only | Flushes the instruction pipeline, ensuring subsequent instructions are fetched with updated translation/privilege. |

### 4.6 SyncEdge Implementation for Inter-Core Signaling on Pi 5

The complete sequence for a VUMA Send operation across cores on the BCM2712:

```asm
// Core 0: Send a message to Core 3 via VUMA SyncEdge
// x0 = message pointer in VUMA_SEND_RING, x1 = doorbell bit (1 << 3)
vuma_send_message:
    // 1. Write message payload (Device-nGnRnE region, strict ordering)
    str     x2, [x0, #0]       // Write payload word 0
    str     x3, [x0, #8]       // Write payload word 1
    str     x4, [x0, #16]      // Write payload word 2
    str     x5, [x0, #24]      // Write payload word 3

    // 2. Ensure payload is visible to all cores (HappensBefore)
    dmb     ish                 // All prior stores visible to inner shareable domain

    // 3. Ring doorbell for target core
    ldr     x6, =0x7C020100    // DOORBELL_SET physical address
    str     w1, [x6]           // Set doorbell bit for Core 3

    // 4. Ensure doorbell write completes (Locked ordering)
    dsb     ish                 // Wait for doorbell store to complete
    isb                        // Synchronize context (ensure IRQ taken)
    ret

// Core 3: Receive message via VUMA SyncEdge (IRQ handler)
vuma_recv_message:
    // 1. Clear doorbell
    ldr     x6, =0x7C020104    // DOORBELL_CLR physical address
    str     w1, [x6]           // Clear doorbell bit for Core 3

    // 2. Acquire ordering for reading message
    ldar    x2, [x0, #0]       // Load payload word 0 with acquire
    ldr     x3, [x0, #8]       // Subsequent loads are ordered after ldar
    ldr     x4, [x0, #16]
    ldr     x5, [x0, #24]

    // 3. Process message (VUMA dispatch)
    b       vuma_dispatch_handler
```

---

## 5. Pi 5 Cache Architecture

### 5.1 Cortex-A76 Cache Hierarchy

The Broadcom BCM2712 integrates four Cortex-A76 cores, each with a private L1 cache and a per-core L2 cache, connected through a coherent interconnect to a shared L3 cache. The complete cache hierarchy is:

| Level | Type | Size | Associativity | Line Size | Latency (cycles) | Scope |
|-------|------|------|---------------|-----------|-----------------|-------|
| L1 I-Cache | Instruction | 64 KB | 4-way | 64 bytes | 3-4 | Per-core private |
| L1 D-Cache | Data | 64 KB | 4-way | 64 bytes | 4 | Per-core private |
| L2 Cache | Unified | 256 KB | 8-way | 64 bytes | 8-12 | Per-core private |
| L3 Cache | Unified | 2 MB | 16-way | 64 bytes | 20-30 | Shared across all 4 cores |

The L2 cache is private to each core (not shared between pairs, as was the case on the older Cortex-A53 cluster in BCM2711). Each core's L2 operates as a strict inclusive cache relative to its L1: all data present in L1 must also be resident in L2. The L3 cache is a victim cache that receives lines evicted from any core's L2 and serves as the coherence point for the four-core cluster.

**Cache coherency** is maintained by the ARM AMBA 5 CHI (Coherent Hub Interface) interconnect within the BCM2712. When Core 0 writes to a cache line that Core 1 has in its L1, the CHI interconnect invalidates Core 1's copy and routes the data. VUMA relies on this hardware coherence for all Inner Shareable Normal memory accesses; no software cache maintenance is required for correctness.

**Cache line size** is 64 bytes at all levels. This is a critical alignment constraint for VUMA: any data structure that may be accessed concurrently by multiple cores must be padded to 64-byte boundaries to prevent false sharing. The VUMA runtime enforces this through its allocator.

### 5.2 Cache Maintenance Operations for VUMA

While hardware coherence handles Normal Inner Shareable memory automatically, VUMA requires explicit cache maintenance for:

1. **Persist heap flush**: Ensuring dirty data in L1/L2/L3 is written back to DRAM before acknowledging a persistence barrier.
2. **Device memory interaction**: Cleaning caches before DMA transfers, invalidating before receiving DMA data.
3. **CapD table updates**: Ensuring newly written capability descriptors are visible to all cores.

The relevant ARM64 cache maintenance instructions are:

| Instruction | Operation | Scope | VUMA Use |
|------------|-----------|-------|----------|
| DC CVAC | Clean data cache by VA to PoC | Per-line | Flush dirty persist data to DRAM |
| DC CIVAC | Clean and invalidate by VA to PoC | Per-line | DMA buffer preparation |
| DC IVAC | Invalidate by VA to PoC | Per-line | Discard stale DMA receive buffers |
| DC CSW | Clean by set/way | Entire cache | Full persist barrier (rare) |
| DC CISW | Clean and invalidate by set/way | Entire cache | Debug/reset only |

Point of Coherence (PoC) on BCM2712 is the DRAM controller interface. Point of Unification (PoU) is the L2 cache, meaning I-cache invalidation after writing code requires DC CVAU + IC IVAU.

```c
// VUMA persist barrier: flush a range to DRAM
static inline void vuma_persist_flush(void *addr, size_t len) {
    uint64_t start = (uint64_t)addr & ~0x3FULL;  // Align to cache line
    uint64_t end   = ((uint64_t)addr + len + 63) & ~0x3FULL;
    for (uint64_t p = start; p < end; p += 64) {
        asm volatile("dc cvac, %0" :: "r"(p) : "memory");
    }
    asm volatile("dsb ish" ::: "memory");  // Wait for all cleans to complete
}
```

### 5.3 VUMA Optimization: Cache-Line Alignment

False sharing occurs when two independent variables share the same 64-byte cache line and are accessed by different cores. The resulting coherence traffic (invalidate + re-fetch) can degrade performance by 10-100x. VUMA's allocator and data structure layout rules prevent false sharing through mandatory cache-line alignment:

```c
// VUMA cache-line aligned allocation
#define VUMA_CACHE_LINE_SIZE  64

// Per-core data structure with automatic padding
typedef struct vuma_core_local {
    uint64_t alloc_ptr;           // Offset 0
    uint64_t alloc_limit;         // Offset 8
    uint64_t rcu_epoch;           // Offset 16
    uint64_t sync_edge_seq;       // Offset 24
    uint8_t  _pad[64 - 32];      // Pad to 64 bytes
} vuma_core_local_t;

static_assert(sizeof(vuma_core_local_t) == VUMA_CACHE_LINE_SIZE,
              "vuma_core_local must be exactly one cache line");
```

VUMA's allocator (`src/pi5/alloc.rs`) guarantees that every allocation of size >= 64 bytes starts at a 64-byte-aligned address. Smaller allocations are grouped into cache-line-sized slabs where all objects in a slab are accessed by the same core.

### 5.4 VUMA Optimization: Structure-of-Arrays (SoA) for Parallel Access

The Cortex-A76's L1 D-cache has 64 KB capacity with 4-way associativity, meaning it can hold 256 cache lines per way. When multiple cores access different fields of the same structure (Array-of-Structs layout), the same cache line carries fields that only one core needs, wasting L1 capacity and causing false sharing. VUMA mandates Structure-of-Arrays (SoA) layout for all multi-core data structures:

**Array-of-Structs (AoS) - AVOIDED by VUMA:**
```
struct vuma_region_aos {
    uint64_t base;       // Core 0 reads
    uint64_t size;       // Core 1 reads
    uint64_t flags;      // Core 2 reads
    uint32_t refcount;   // Core 3 writes
    uint32_t _pad;
    uint8_t  capd[32];   // Core 0 writes
    uint8_t  _pad2[8];
};  // 64 bytes - all cores touch this cache line!
```

**Structure-of-Arrays (SoA) - MANDATED by VUMA:**
```
struct vuma_region_soa {
    uint64_t bases[N];       // Contiguous cache lines - Core 0 only
    uint64_t sizes[N];       // Contiguous cache lines - Core 1 only
    uint64_t flags[N];       // Contiguous cache lines - Core 2 only
    uint32_t refcounts[N];   // Contiguous cache lines - Core 3 only
    uint8_t  capds[N][32];   // Contiguous cache lines - Core 0 only
};
```

With SoA layout, each core's working set fits entirely within its private L1 D-cache, avoiding L2/L3 traffic. For the VUMA Region table on a Pi 5 with N=1024 active regions:
- AoS: Each core touches all 1024 cache lines (64 KB total), evicting its entire L1 D-cache.
- SoA: Each core touches ~256 cache lines (16 KB), fitting comfortably in L1 with room to spare.

The VUMA code generator (`src/codegen`) automatically transforms AoS declarations to SoA layout when the `#[vuma_soa]` attribute is present:

```rust
#[vuma_soa]
struct Region {
    base: u64,
    size: u64,
    flags: u64,
    refcount: AtomicU32,
    capd: [u8; 32],
}
```

### 5.5 Prefetch Strategy for VUMA on Cortex-A76

The Cortex-A76 supports both hardware prefetch (automatic) and software prefetch via the PRFM instruction. VUMA leverages software prefetch for predictable access patterns in the capability table and persist heap:

```asm
// Prefetch VUMA capability table entries during scan
vuma_cap_scan:
    mov     x2, #0              // Index
    adrp    x0, vuma_cap_table
.loop:
    prfm    pldl1keep, [x0, x2, lsl #6]   // Prefetch CapD[i] into L1 (keep)
    add     x2, x2, #1
    cmp     x2, #16             // Prefetch 16 entries ahead
    b.lt    .loop
    // ... process entries ...
```

The PRFM hint types used by VUMA:

| PRFM Type | Meaning | VUMA Use |
|-----------|---------|----------|
| `pldl1keep` | Prefetch to L1, keep in cache | CapD scan, heap traversal |
| `pldl1strm` | Prefetch to L1, streaming (evict soon) | Linear persist heap flush |
| `pldl2keep` | Prefetch to L2, keep | Cross-core shared data |
| `pldl3keep` | Prefetch to L3, keep | Infrequently shared data |

### 5.6 TLB Considerations for VUMA

The Cortex-A76 implements separate TLBs for instruction and data accesses:

| TLB | Entries | Granule | Scope |
|-----|---------|---------|-------|
| L1 ITLB | 48 | 4 KB / 16 KB / 64 KB | Per-core |
| L1 DTLB | 48 | 4 KB / 16 KB / 64 KB | Per-core |
| L2 STLB (Unified) | 1280 | 4 KB / 16 KB / 64 KB | Per-core |

The L2 STLB (Shared TLB) is per-core but holds 1280 entries, providing coverage for 5 MB of 4 KB pages or 80 MB of 64 KB pages. VUMA's capability table (16 MB) requires 4096 page table entries, which exceeds the L2 STLB capacity. To avoid TLB thrashing, VUMA maps the capability table using 2 MB huge pages (PMD-level block mappings), reducing the TLB footprint to just 8 entries:

```c
// Map CapD table using 2 MB block entries (PMD level)
for (int i = 0; i < 8; i++) {
    pmd[i] = (0x0E000000 + i * 0x200000)  // Physical address
           | (1ULL << 0)   // Valid
           | (1ULL << 1)   // Page (block mapping at PMD level)
           | (0b00ULL << 6) // AP: read-only at EL0
           | (0b000ULL << 2) // AttrIndx: Normal WB-WA
           | (0b11ULL << 8)  // Inner Shareable
           | (1ULL << 10);   // AF (access flag)
}
```

TLB maintenance instructions required by VUMA:

| Instruction | Purpose | When Used |
|------------|---------|-----------|
| TLBI VMALLE1IS | Invalidate all user-space TLB entries (inner-shareable) | CapD table resize |
| TLBI VAE1IS, x0 | Invalidate specific VA in user-space TLB | Single page CapD update |
| TLBI VMALLE1 | Invalidate all user-space TLB entries (local) | Context switch |
| TLBI ALLE2IS | Invalidate all hypervisor TLB entries | Bare-metal mode only |

After any TLBI instruction, VUMA always executes `DSB ISH` followed by `ISB` to ensure the invalidation is visible to subsequent memory accesses and instruction fetches.

---

## Appendix A: Register Quick Reference

### System Registers for VUMA Memory Management

| Register | Purpose | Key Fields |
|----------|---------|------------|
| SCTLR_EL1 | System Control | M (MMU enable), C (data cache enable), I (I-cache enable) |
| TCR_EL1 | Translation Control | T0SZ, T1SZ, TG0, TG1, SH0, SH1, IRGN0, ORGN0, IPS, HA, HD |
| TTBR0_EL1 | Translation Table Base 0 | BADDR[47:1] (PGD physical address for user space) |
| TTBR1_EL1 | Translation Table Base 1 | BADDR[47:1] (PGD physical address for kernel space) |
| MAIR_EL1 | Memory Attribute Indirection | Attr0-Attr7 (8 attribute entries, each 8 bits) |
| ID_AA64MMFR0_EL1 | Memory Model Feature 0 | PARange, ASIDBits, BigEnd, LVA, TGran |
| ID_AA64ISAR0_EL1 | Instruction Set Feature 0 | Atomic (LSE support), CRC32, SHA1, SHA256 |
| PAR_EL1 | Physical Address Result | Output of AT instruction queries |

### BCM2712 Physical Address Quick Reference

| Peripheral | Physical Base | Size |
|-----------|--------------|------|
| DMA Channels | `0x7C100000` | 64 KB |
| GPIO Controller | `0x7C200000` | 64 KB |
| UART0 (PL011) | `0x7C201000` | 4 KB |
| SPI0 | `0x7C204000` | 4 KB |
| I2C0 | `0x7C205000` | 4 KB |
| PWM0 | `0x7C20C000` | 4 KB |
| Mailbox / Doorbell | `0x7C020000` | 4 KB |
| PCIe ECAM | `0x100000000` | 4 KB |
| PCIe MMIO | `0x100020000` | ~1 GB |
| System Timer | `0x7C003000` | 4 KB |
| Interrupt Controller (GIC-400) | `0x7C400000` | 64 KB |
| Watchdog | `0x7C100000` | — |
| RPiVid (Video Decoder) | `0x7EB00000` | 1 MB |
| HDMI TX0 | `0x7EF00000` | 64 KB |

---

## Appendix B: VUMA CapD Permission Bit Encoding

```
CapD (64 bytes):
  [0:7]   permissions bitmask
    bit 0: cap_read    - Read access
    bit 1: cap_write   - Write access
    bit 2: cap_exec    - Execute access
    bit 3: cap_persist - Persistent (durable) storage
    bit 4: cap_send    - Send (messaging) channel
    bit 5: cap_device  - Device MMIO mapping
    bit 6: cap_alias   - Alias (derived capability)
    bit 7: cap_revoked - Revocation pending flag
  [8:15]  region_type (0=heap, 1=stack, 2=persist, 3=send, 4=device, 5=cap_table)
  [16:23] SyncEdge ordering (0=relaxed, 1=acquire, 2=release, 3=acqrel, 4=happensbefore, 5=atomic, 6=locked)
  [24:31] core_affinity mask (bits 0-3 for cores 0-3)
  [32:39] shareability (0=non-shareable, 2=outer, 3=inner)
  [40:47] reserved
  [48:63] region_id (16-bit unique identifier)
  [0x10:0x17] base_address (virtual address of Region start)
  [0x18:0x1F] region_size (size of Region in bytes)
  [0x20:0x27] physical_base (corresponding physical address)
  [0x28:0x2F] parent_capd (index of parent CapD, or 0xFFFF for root)
  [0x30:0x37] epoch (monotonic version counter for revocation)
  [0x38:0x3F] reserved_for_hardware
```

---

*End of VUMA Pi 5 Memory Model Specification, Revision 1.0*
