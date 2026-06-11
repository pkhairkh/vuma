//! # vuma-pi5 — Raspberry Pi 5 backend for the VUMA hypervisor
//!
//! This crate provides bare-metal hardware abstraction for the
//! Raspberry Pi 5 (BCM2712 SoC, 4× Cortex-A76). It is designed to be
//! used as a backend module within the VUMA hypervisor project and
//! targets `aarch64` bare-metal environments.
//!
//! # Module overview
//!
//! | Module      | Description                                     |
//! |-------------|-------------------------------------------------|
//! | [`boot`]    | Bare-metal boot, exception vectors, FDT parsing |
//! | [`mmio`]    | Memory-mapped I/O, barriers, `MmioDevice` trait |
//! | [`mmu`]     | MMU and page table initialization              |
//! | [`platform`]| Platform constants, memory map, `Platform` trait|
//! | [`gpio`]    | GPIO pin configuration and I/O                  |
//! | [`uart`]    | PL011 UART serial driver                        |
//! | [`timer`]   | ARM Generic Timer (CNTPCT_EL0 / CNTFRQ_EL0)    |
//! | [`mailbox`] | BCM2712 VideoCore mailbox (property messages)   |
//! | [`power`]   | Power management (device power, reboot, off)    |
//! | [`smp`]     | Multi-core start-up and inter-core messaging    |
//!
//! # Feature flags
//!
//! This crate currently has no optional feature flags. All modules are
//! always available.
//!
//! # Usage
//!
//! ```no_run
//! use vuma_pi5::platform::Pi5Platform;
//! use vuma_pi5::uart::Uart;
//! use vuma_pi5::timer::Timer;
//!
//! let platform = Pi5Platform::default_platform();
//! let uart = Uart::new(platform.uart_base());
//! uart.init(115200);
//! uart.write_str("Hello from Pi 5!\n");
//!
//! let timer = Timer::new();
//! timer.delay_ms(100);
//! ```

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

// Link std in test builds so that unit tests can use HashMap, RefCell, etc.
#[cfg(test)]
extern crate std;

#[cfg(target_arch = "aarch64")]
pub mod boot;
pub mod exception;
pub mod gic;
pub mod gpio;
pub mod mailbox;
pub mod mmio;
pub mod mmu;
pub mod platform;
pub mod power;
#[cfg(target_arch = "aarch64")]
pub mod smp;
#[cfg(target_arch = "aarch64")]
pub mod timer;
pub mod uart;

// Re-export commonly used types at the crate root for convenience.
pub use exception::{ExceptionContext, ExceptionType};
pub use gic::Gic400;
pub use mmio::{Address, MmioDevice};
pub use platform::{Pi5Platform, Platform};
