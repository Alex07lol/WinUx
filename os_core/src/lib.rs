//! `os_core` — architecture-independent logic shared by the PROJECT_OS kernel
//! and host-side tests.
//!
//! Everything in this crate is `no_std` when compiled for the kernel and uses
//! only ordinary Rust when compiled for the host test harness. It deliberately
//! contains no pointer writes to hardware: memory-map accounting, framebuffer
//! geometry/colour math and the physical-frame bitmap allocator can therefore
//! be exercised on a development machine before they ever touch a VM.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]

extern crate alloc;

pub mod config;
pub mod driver;
pub mod elf;
pub mod fb;
pub mod font;
pub mod frame;
pub mod heap;
pub mod memmap;
pub mod net;
pub mod paging;
pub mod path;
pub mod pci;
pub mod pe;
pub mod pofs;
pub mod rng;
pub mod textcon;
pub mod textrender;

/// Temporary internal project name. Branding is intentionally not part of the
/// boot milestone.
pub const PROJECT_NAME: &str = "PROJECT_OS";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Limine boot protocol base revision implemented by the kernel.
///
/// Revision 6 is the newest revision published by the Limine protocol at the
/// time of writing; on x86-64 it has the same entry guarantees as revision 5.
pub const LIMINE_BASE_REVISION: u64 = 6;

/// Link-time virtual base of the kernel image (see `kernel/linker.ld`).
pub const KERNEL_VIRTUAL_BASE: u64 = 0xffff_ffff_8000_0000;
