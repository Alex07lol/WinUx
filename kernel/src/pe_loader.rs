//! Two-phase PE32+ loader for ring-3 programs shipped as Limine modules
//! (Phase 7, stage 1).
//!
//! [`inspect`] validates the module without touching memory: the shared
//! `os_core::pe` parser enforces the format rules and this module adds the
//! kernel policy — every section's page-rounded range must lie inside the
//! fixed code window, W^X must hold per section and a nonzero load delta
//! requires relocations. [`map`] then allocates, zero-fills, maps and
//! base-relocates each section page into a fresh user address space, rolling
//! the whole mapping back on any failure. The stack is owned by `user.rs`.
//!
//! With the ELF loader this shares the fixed 1 MiB window at `0x400000`: one
//! program runs at a time, so the two loaders cannot collide.

use core::ops::Range;

use os_core::frame::PhysFrame;
use os_core::paging::{NO_EXECUTE, PAGE_SIZE, PRESENT, USER, USER_CODE, USER_DATA};
use os_core::pe::{PeError, PeImage, Section};

use crate::limine;
use crate::loader;
use crate::memory;
use crate::vm::{self, AddressSpace};

/// Kernel load caps, stricter than the parser's (`os_core::pe`): 4 KiB pages
/// per section stay bounded and boot time stays predictable.
pub const MAX_SECTIONS: usize = 16;
pub const MAX_RELOCATIONS: usize = 256;

/// Worst-case rollback bookkeeping: every page of the code window.
const MAX_WINDOW_PAGES: usize = ((loader::CODE_END - loader::CODE_BASE) / PAGE_SIZE) as usize;

/// Why a module was rejected. Labels are stable and logged at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Pe(PeError),
    OutsideWindow,
    TooManySections,
    WriteExecuteSection,
    StrippedRelocations,
    RelocationTarget,
    OutOfMemory,
    MapFailed,
}

impl Reject {
    pub fn label(&self) -> &'static str {
        match self {
            Reject::Pe(error) => error.label(),
            Reject::OutsideWindow => "outside code window",
            Reject::TooManySections => "too many sections",
            Reject::WriteExecuteSection => "write+execute section",
            Reject::StrippedRelocations => "relocations stripped but a rebase is required",
            Reject::RelocationTarget => "relocation target invalid",
            Reject::OutOfMemory => "out of memory",
            Reject::MapFailed => "mapping failed",
        }
    }
}

/// Validated load plan for one PE module: entry RVA, link-time image base and
/// the sections/relocations that passed kernel policy.
pub struct Inspection {
    image_base: u64,
    entry_rva: u32,
    sections: [Section; MAX_SECTIONS],
    count: usize,
    relocations: [u32; MAX_RELOCATIONS],
    relocation_count: usize,
}

impl Inspection {
    pub fn entry_rva(&self) -> u32 {
        self.entry_rva
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections[..self.count]
    }
}

/// Absolute user-virtual page range covered by one section (page-rounded on
/// both ends; section alignment may be smaller than a page).
pub fn section_page_range(section: &Section) -> Range<u64> {
    let start = align_down(u64::from(section.virtual_address));
    let end = align_up(u64::from(section.virtual_address) + section.memory_size());
    loader::CODE_BASE + start..loader::CODE_BASE + end
}

fn align_down(value: u64) -> u64 {
    value & !(PAGE_SIZE - 1)
}

fn align_up(value: u64) -> u64 {
    (value + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

/// Parse and policy-check a module without allocating anything.
pub fn inspect(module: &limine::File) -> Result<Inspection, Reject> {
    let bytes = loader::module_bytes(module);
    let pe = PeImage::parse(bytes).map_err(Reject::Pe)?;
    if pe.sections().len() > MAX_SECTIONS {
        return Err(Reject::TooManySections);
    }
    for section in pe.sections() {
        if section.is_executable() && section.is_writable() {
            return Err(Reject::WriteExecuteSection);
        }
        let range = section_page_range(section);
        if range.end < range.start || range.end > loader::CODE_END {
            return Err(Reject::OutsideWindow);
        }
    }
    // The fixed window base is never the link-time base in this stage, so the
    // image must carry relocations for the delta to be applied.
    if loader::CODE_BASE != pe.image_base()
        && (pe.relocation_stripped() || pe.relocations().is_empty())
    {
        return Err(Reject::StrippedRelocations);
    }

    let mut sections = [Section::EMPTY; MAX_SECTIONS];
    for (index, section) in pe.sections().iter().enumerate() {
        sections[index] = *section;
    }
    let mut relocations = [0u32; MAX_RELOCATIONS];
    for (index, relocation) in pe.relocations().iter().enumerate() {
        relocations[index] = *relocation;
    }
    Ok(Inspection {
        image_base: pe.image_base(),
        entry_rva: pe.entry_rva(),
        sections,
        count: pe.sections().len(),
        relocations,
        relocation_count: pe.relocations().len(),
    })
}

/// Map every section page into `space`, copy the file payload, apply the
/// DIR64 base relocations and return how many were applied. On any failure
/// every page mapped so far is unmapped and its frame freed.
pub fn map(
    space: &mut AddressSpace,
    module: &limine::File,
    image: &Inspection,
) -> Result<u32, Reject> {
    let Some(hhdm) = vm::hhdm_offset() else {
        return Err(Reject::MapFailed);
    };
    let bytes = loader::module_bytes(module);
    let mut mapped = [0u64; MAX_WINDOW_PAGES];
    let mut mapped_count = 0usize;

    for section in image.sections() {
        let range = section_page_range(section);
        let payload_start = loader::CODE_BASE + u64::from(section.virtual_address);
        let payload_end = payload_start + u64::from(section.raw_size);
        let mut page = range.start;
        while page < range.end {
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
            let copy_start = page.max(payload_start);
            let copy_end = (page + PAGE_SIZE).min(payload_end);
            if copy_start < copy_end {
                let source = u64::from(section.raw_offset) + (copy_start - payload_start);
                let length = (copy_end - copy_start) as usize;
                let offset = (copy_start - page) as usize;
                // SAFETY: `raw_offset + raw_size <= len` was proved by the
                // parser, so `source..source + length` is inside `bytes`.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(source as usize),
                        window.add(offset),
                        length,
                    )
                };
            }

            let flags = if section.is_executable() {
                USER_CODE
            } else if section.is_writable() {
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

    let delta = loader::CODE_BASE.wrapping_sub(image.image_base);
    for index in 0..image.relocation_count {
        let rva = u64::from(image.relocations[index]);
        let target_section = image.sections().iter().find(|section| {
            let range = section.rva_range();
            section.is_writable() && rva >= range.start && rva + 8 <= range.end
        });
        if target_section.is_none() || rva % PAGE_SIZE > PAGE_SIZE - 8 {
            rollback(space, &mapped[..mapped_count]);
            return Err(Reject::RelocationTarget);
        }
        let target = loader::CODE_BASE + rva;
        let Some((physical, _)) = space.translate(target) else {
            rollback(space, &mapped[..mapped_count]);
            return Err(Reject::RelocationTarget);
        };
        let Some(cell) = hhdm.checked_add(physical) else {
            rollback(space, &mapped[..mapped_count]);
            return Err(Reject::MapFailed);
        };
        let cell = cell as *mut u64;
        // SAFETY: the target is an 8-byte, in-page, writable mapped address;
        // the HHDM window for its frame is accessible in the kernel.
        unsafe {
            let value = core::ptr::read_volatile(cell);
            core::ptr::write_volatile(cell, value.wrapping_add(delta));
        }
        vm::flush(target);
    }
    Ok(image.relocation_count as u32)
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
