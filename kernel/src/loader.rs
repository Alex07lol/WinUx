//! Two-phase ELF64 loader for ring-3 programs shipped as Limine modules
//! (Phase 6, stage 3).
//!
//! [`inspect`] validates the module without touching memory: the shared
//! `os_core::elf` parser enforces the ELF-level rules and this module adds the
//! kernel policy — every `PT_LOAD` page must lie inside the fixed code window.
//! [`map`] then allocates, zero-fills and maps each page into a fresh user
//! address space, rolling the whole mapping back on any failure. The stack is
//! owned by `user.rs`, not the loader.

use os_core::elf::{ElfImage, Segment};
use os_core::frame::PhysFrame;
use os_core::paging::{NO_EXECUTE, PAGE_SIZE, PRESENT, USER, USER_CODE, USER_DATA};

use crate::limine;
use crate::memory;
use crate::vm::{self, AddressSpace};

/// 1 MiB user code window, page-aligned and disjoint from the stack page.
pub const CODE_BASE: u64 = 0x40_0000;
pub const CODE_END: u64 = 0x50_0000;
/// The parser allows up to 64 program headers; the loader caps loaded
/// segments so `Inspection` stays small and boot time bounded.
pub const MAX_SEGMENTS: usize = 16;

/// Every page in the code window, the worst-case rollback bookkeeping size.
const MAX_WINDOW_PAGES: usize = ((CODE_END - CODE_BASE) / PAGE_SIZE) as usize;

/// Why a module was rejected. Labels are stable and logged at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Elf(os_core::elf::ElfError),
    OutsideWindow,
    TooManySegments,
    OutOfMemory,
    MapFailed,
}

impl Reject {
    pub fn label(&self) -> &'static str {
        match self {
            Reject::Elf(error) => error.label(),
            Reject::OutsideWindow => "outside code window",
            Reject::TooManySegments => "too many segments",
            Reject::OutOfMemory => "out of memory",
            Reject::MapFailed => "mapping failed",
        }
    }
}

/// Validated load plan for one module: entry point plus the `PT_LOAD`
/// segments (page ranges all inside the code window).
#[derive(Clone, Copy)]
pub struct Inspection {
    entry: u64,
    segments: [Segment; MAX_SEGMENTS],
    count: usize,
}

impl Inspection {
    pub fn entry(&self) -> u64 {
        self.entry
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.count]
    }
}

/// The module content as a byte slice. Module addresses are HHDM-virtual and
/// valid for the kernel lifetime; a null address or zero size yields an empty
/// slice, which the parser rejects as truncated.
fn module_bytes(module: &limine::File) -> &'static [u8] {
    if module.address.is_null() || module.size == 0 {
        return &[];
    }
    // SAFETY: the protocol guarantees `address..address + size` is a valid,
    // 4 KiB-aligned, exclusively-owned module mapping.
    unsafe { core::slice::from_raw_parts(module.address, module.size as usize) }
}

/// Parse and policy-check a module without allocating anything.
pub fn inspect(module: &limine::File) -> Result<Inspection, Reject> {
    let bytes = module_bytes(module);
    let elf = ElfImage::parse(bytes).map_err(Reject::Elf)?;
    let mut segments = [Segment {
        offset: 0,
        vaddr: 0,
        filesz: 0,
        memsz: 0,
        flags: 0,
    }; MAX_SEGMENTS];
    let mut count = 0usize;
    for segment in elf.program_headers() {
        if count == MAX_SEGMENTS {
            return Err(Reject::TooManySegments);
        }
        let range = segment.page_range();
        if range.start < CODE_BASE || range.end > CODE_END {
            return Err(Reject::OutsideWindow);
        }
        segments[count] = segment;
        count += 1;
    }
    // `parse` already proved a `PT_LOAD` segment contains the entry.
    Ok(Inspection {
        entry: elf.entry(),
        segments,
        count,
    })
}

/// Map every segment page into `space`: allocate a frame, zero it through the
/// HHDM, copy the file payload, map with W^X-safe flags. On any failure every
/// page mapped so far is unmapped and its frame freed before returning.
pub fn map(
    space: &mut AddressSpace,
    module: &limine::File,
    image: &Inspection,
) -> Result<(), Reject> {
    let Some(hhdm) = vm::hhdm_offset() else {
        return Err(Reject::MapFailed);
    };
    let bytes = module_bytes(module);
    let mut mapped = [0u64; MAX_WINDOW_PAGES];
    let mut mapped_count = 0usize;

    for segment in image.segments() {
        let page_end = segment.page_range().end;
        let file_end = segment.vaddr + segment.filesz;
        let mut page = segment.vaddr;
        while page < page_end {
            let frame = match memory::alloc_frame() {
                Ok(frame) => frame,
                Err(_) => {
                    rollback(space, &mapped[..mapped_count]);
                    return Err(Reject::OutOfMemory);
                }
            };
            let Some(window) = hhdm.checked_add(frame.address()) else {
                let _ = memory::free_pages(frame, 1);
                rollback(space, &mapped[..mapped_count]);
                return Err(Reject::MapFailed);
            };
            let window = window as *mut u8;

            // SAFETY: the frame was just allocated for this slot and is not
            // mapped anywhere; HHDM maps all of usable RAM.
            unsafe { core::ptr::write_bytes(window, 0, PAGE_SIZE as usize) };
            if page < file_end {
                let source = segment.offset + (page - segment.vaddr);
                let length = (file_end - page).min(PAGE_SIZE) as usize;
                // SAFETY: the parser proved `offset + filesz <= len`, so the
                // bounded range `source..source + length` is inside `bytes`.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(source as usize),
                        window,
                        length,
                    )
                };
            }

            let flags = if segment.is_executable() {
                USER_CODE
            } else if segment.is_writable() {
                USER_DATA
            } else {
                PRESENT | USER | NO_EXECUTE
            };
            if space.map_page(page, frame.address(), flags).is_err() {
                let _ = memory::free_pages(frame, 1);
                rollback(space, &mapped[..mapped_count]);
                return Err(Reject::MapFailed);
            }
            mapped[mapped_count] = page;
            mapped_count += 1;
            page += PAGE_SIZE;
        }
    }
    Ok(())
}

/// Best-effort unmapping and frame freeing for pages mapped by a failed
/// `map` call. The caller still destroys the (now empty) address space.
fn rollback(space: &mut AddressSpace, mapped: &[u64]) {
    for &virtual_address in mapped {
        if let Ok(physical) = space.unmap_page(virtual_address)
            && let Some(frame) = PhysFrame::from_address(physical)
        {
            let _ = memory::free_pages(frame, 1);
        }
    }
}
