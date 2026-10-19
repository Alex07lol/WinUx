//! Two-phase PE32+ loader for ring-3 programs shipped as Limine modules
//! (Phase 7, stages 1–3).
//!
//! [`inspect`] validates the module without touching memory: the shared
//! `os_core::pe` parser enforces the format rules and this module adds the
//! kernel policy — every section's page-rounded range must lie inside the
//! fixed code window, W^X must hold per section, a nonzero load delta
//! requires relocations, and every import must resolve against the fixed
//! Win32 allowlist (exact `kernel32.dll` or `advapi32.dll` name and one of
//! fifteen function names). [`map`] then allocates, zero-fills, maps and
//! base-relocates each section page into a fresh user address space and, when
//! the image has imports, maps one stub page of `int 0x80` trampolines
//! (4-, 5- and 6-argument shapes; the fifth integer argument is forwarded
//! through `r10` and the sixth through `r11`) and patches the IAT through the
//! HHDM, rolling the whole mapping back on any failure. The same page carries
//! the loader-written command-line string that `GetCommandLineA` returns. The
//! stack is owned by `user.rs`.
//!
//! With the ELF loader this shares the fixed 1 MiB window at `0x400000`: one
//! program runs at a time, so the two loaders cannot collide. The import stub
//! page sits at [`WIN32_STUB_VA`], just above the stack page.

use core::ops::Range;

use os_core::frame::PhysFrame;
use os_core::paging::{NO_EXECUTE, PAGE_SIZE, PRESENT, USER, USER_CODE, USER_DATA};
use os_core::pe::{ImportSlot, MAX_IMPORTS, PeError, PeImage, Section};

use crate::limine;
use crate::loader;
use crate::memory;
use crate::syscall;
use crate::vm::{self, AddressSpace};

/// Kernel load caps, stricter than the parser's (`os_core::pe`): 4 KiB pages
/// per section stay bounded and boot time stays predictable.
pub const MAX_SECTIONS: usize = 16;
pub const MAX_RELOCATIONS: usize = 256;

/// The DLLs this stage resolves imports against, byte-exact. Since Phase 8
/// stage 3 the allowlist spans two DLLs: console/program services come from
/// `kernel32.dll`, the read-only registry surface from `advapi32.dll`.
const WIN32_DLLS: [&[u8]; 2] = [b"kernel32.dll", b"advapi32.dll"];

/// Fixed import stub page: outside the code window (`loader::CODE_END`) and
/// disjoint from the user stack page at `0x50_0000`.
pub const WIN32_STUB_VA: u64 = 0x51_0000;

/// One 24-byte stub per unique import (an `Args6` stub needs 18 bytes);
/// unused bytes are `int3`.
const STUB_STRIDE: u64 = 24;
const STUB_UNUSED: u8 = 0xCC;

/// Command-line string for `GetCommandLineA`: a fixed 256-byte tail region
/// of the stub page (255 path bytes + NUL terminator).
const COMMAND_LINE_OFFSET: usize = 0xF00;
pub const COMMAND_LINE_CAPACITY: usize = 0x100;
pub const COMMAND_LINE_VA: u64 = WIN32_STUB_VA + COMMAND_LINE_OFFSET as u64;
const _: () = assert!(COMMAND_LINE_OFFSET + COMMAND_LINE_CAPACITY <= PAGE_SIZE as usize);
const _: () = assert!(MAX_IMPORTS as u64 * STUB_STRIDE <= COMMAND_LINE_OFFSET as u64);

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
    UnknownImportDll,
    UnknownImportFunction,
    ImportTarget,
    CommandLineTooLong,
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
            Reject::UnknownImportDll => "unknown import dll",
            Reject::UnknownImportFunction => "unknown import function",
            Reject::ImportTarget => "import target invalid",
            Reject::CommandLineTooLong => "command line too long",
            Reject::OutOfMemory => "out of memory",
            Reject::MapFailed => "mapping failed",
        }
    }
}

/// How many integer arguments an import's stub must forward. `Args4` covers
/// zero to four arguments: the services ignore unused registers. `Args6`
/// forwards the sixth argument (at `[rsp+0x30]` on entry) through `r11`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StubShape {
    Args4,
    Args5,
    Args6,
}

/// The fixed Win32 service table of this stage: DLL and function name
/// (byte-exact) to `int 0x80` service number and stub shape. This single
/// table backs both the import policy and the stub bytes.
const WIN32_SERVICES: [(&[u8], &[u8], u32, StubShape); 15] = [
    (
        b"kernel32.dll",
        b"GetStdHandle",
        syscall::SYS_GET_STD_HANDLE as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"WriteConsoleA",
        syscall::SYS_WRITE_CONSOLE as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"ExitProcess",
        syscall::SYS_EXIT_PROCESS as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"WriteFile",
        syscall::SYS_WRITEFILE as u32,
        StubShape::Args5,
    ),
    (
        b"kernel32.dll",
        b"GetLastError",
        syscall::SYS_GET_LAST_ERROR as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"SetLastError",
        syscall::SYS_SET_LAST_ERROR as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"ExitThread",
        syscall::SYS_EXIT_THREAD as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"GetCurrentProcessId",
        syscall::SYS_GET_CURRENT_PROCESS_ID as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"GetCurrentThreadId",
        syscall::SYS_GET_CURRENT_THREAD_ID as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"GetTickCount",
        syscall::SYS_GET_TICK_COUNT as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"GetCommandLineA",
        syscall::SYS_GET_COMMAND_LINE as u32,
        StubShape::Args4,
    ),
    (
        b"kernel32.dll",
        b"ReadFile",
        syscall::SYS_READFILE as u32,
        StubShape::Args5,
    ),
    (
        b"advapi32.dll",
        b"RegOpenKeyExA",
        syscall::SYS_REG_OPEN_KEY_EX as u32,
        StubShape::Args5,
    ),
    (
        b"advapi32.dll",
        b"RegQueryValueExA",
        syscall::SYS_REG_QUERY_VALUE_EX as u32,
        StubShape::Args6,
    ),
    (
        b"advapi32.dll",
        b"RegCloseKey",
        syscall::SYS_REG_CLOSE_KEY as u32,
        StubShape::Args4,
    ),
];

fn win32_service(dll: &[u8], name: &[u8]) -> Option<(u32, StubShape)> {
    WIN32_SERVICES
        .iter()
        .find(|(candidate_dll, candidate_name, _, _)| {
            *candidate_dll == dll && *candidate_name == name
        })
        .map(|(_, _, service, shape)| (*service, *shape))
}

/// Validated load plan for one PE module: entry RVA, link-time image base and
/// the sections, relocations and imports that passed kernel policy.
#[derive(Clone, Copy)]
pub struct Inspection {
    image_base: u64,
    entry_rva: u32,
    sections: [Section; MAX_SECTIONS],
    count: usize,
    relocations: [u32; MAX_RELOCATIONS],
    relocation_count: usize,
    imports: [ImportSlot; MAX_IMPORTS],
    import_count: usize,
}

impl Inspection {
    pub fn entry_rva(&self) -> u32 {
        self.entry_rva
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections[..self.count]
    }
}

/// Everything `map` produced: the validated plan (owned), the import stub
/// page VA (`0` when the image has no imports) and the number of DIR64
/// fixups applied.
pub struct Loaded {
    pub image: Inspection,
    pub stub_page: u64,
    pub applied_relocations: u32,
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

    // Import policy: exact-name allowlist, and every IAT slot must be a
    // patchable 8-byte, in-page location inside a readable section of the
    // code window (the loader writes through the HHDM, so the section may
    // stay read-only to the user program).
    for import in pe.imports() {
        if !WIN32_DLLS.iter().any(|dll| *dll == import.dll()) {
            return Err(Reject::UnknownImportDll);
        }
        if win32_service(import.dll(), import.name()).is_none() {
            return Err(Reject::UnknownImportFunction);
        }
        let slot = u64::from(import.iat_rva);
        let readable_owner = pe.sections().iter().any(|section| {
            let range = section.rva_range();
            section.is_readable() && slot >= range.start && slot + 8 <= range.end
        });
        if !readable_owner
            || align_up(slot + 8) > loader::CODE_END - loader::CODE_BASE
            || slot % PAGE_SIZE > PAGE_SIZE - 8
        {
            return Err(Reject::ImportTarget);
        }
    }

    let mut sections = [Section::EMPTY; MAX_SECTIONS];
    for (index, section) in pe.sections().iter().enumerate() {
        sections[index] = *section;
    }
    let mut relocations = [0u32; MAX_RELOCATIONS];
    for (index, relocation) in pe.relocations().iter().enumerate() {
        relocations[index] = *relocation;
    }
    let mut imports = [ImportSlot::EMPTY; MAX_IMPORTS];
    for (index, import) in pe.imports().iter().enumerate() {
        imports[index] = *import;
    }
    Ok(Inspection {
        image_base: pe.image_base(),
        entry_rva: pe.entry_rva(),
        sections,
        count: pe.sections().len(),
        relocations,
        relocation_count: pe.relocations().len(),
        imports,
        import_count: pe.imports().len(),
    })
}

/// Map every section page into `space`, copy the file payload, apply the
/// DIR64 base relocations, then — when the image has imports — map the stub
/// page and patch every IAT slot. On any failure every page mapped so far is
/// unmapped and its frame freed.
pub fn map(
    space: &mut AddressSpace,
    module: &limine::File,
    image: &Inspection,
) -> Result<Loaded, Reject> {
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

    let mut stub_page = 0u64;
    if image.import_count > 0 {
        // One stub per unique function name, first occurrence order; `inspect`
        // already proved every name resolves, so planning cannot fail here.
        let mut stub_index = [0u8; MAX_IMPORTS];
        let mut stub_services = [0u32; MAX_IMPORTS];
        let mut stub_shapes = [StubShape::Args4; MAX_IMPORTS];
        let mut stub_count = 0usize;
        for (slot, import) in image.imports[..image.import_count].iter().enumerate() {
            match (0..stub_count).find(|&index| {
                image.imports[index].dll() == import.dll()
                    && image.imports[index].name() == import.name()
            }) {
                Some(index) => stub_index[slot] = index as u8,
                None => {
                    let Some((service, shape)) = win32_service(import.dll(), import.name()) else {
                        rollback(space, &mapped[..mapped_count]);
                        return Err(Reject::UnknownImportFunction);
                    };
                    stub_index[slot] = stub_count as u8;
                    stub_services[stub_count] = service;
                    stub_shapes[stub_count] = shape;
                    stub_count += 1;
                }
            }
        }

        // The command line is the module's real Limine path; both the
        // fixture and the kernel log read it back through the mapping. The
        // fixed capacity is checked before any stub-page frame is allocated.
        let command_line = limine::cstr(module.path);
        if command_line.len() + 1 > COMMAND_LINE_CAPACITY {
            rollback(space, &mapped[..mapped_count]);
            return Err(Reject::CommandLineTooLong);
        }

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
        // SAFETY: the frame was just allocated for this stub page and is not
        // mapped anywhere; HHDM maps all of usable RAM. Unused bytes are
        // `int3`, so a stray call faults in isolation.
        unsafe { core::ptr::write_bytes(window, STUB_UNUSED, PAGE_SIZE as usize) };
        for index in 0..stub_count {
            let service = stub_services[index];
            let mut stub = [STUB_UNUSED; STUB_STRIDE as usize];
            let mut cursor = 0usize;
            if stub_shapes[index] == StubShape::Args5 || stub_shapes[index] == StubShape::Args6 {
                // mov r10, [rsp+0x28]: the fifth integer argument at
                // function entry (32-byte shadow space + return address).
                stub[cursor..cursor + 5].copy_from_slice(&[0x4C, 0x8B, 0x54, 0x24, 0x28]);
                cursor += 5;
            }
            if stub_shapes[index] == StubShape::Args6 {
                // mov r11, [rsp+0x30]: the sixth integer argument.
                stub[cursor..cursor + 5].copy_from_slice(&[0x4C, 0x8B, 0x5C, 0x24, 0x30]);
                cursor += 5;
            }
            stub[cursor] = 0xB8; // mov eax, imm32
            stub[cursor + 1..cursor + 5].copy_from_slice(&service.to_le_bytes());
            stub[cursor + 5] = 0xCD; // int 0x80
            stub[cursor + 6] = 0x80;
            stub[cursor + 7] = 0xC3; // ret
            for byte in &mut stub[cursor + 8..] {
                *byte = 0x90; // nop
            }
            // SAFETY: `index * STUB_STRIDE + STUB_STRIDE` stays inside the
            // one frame (`MAX_IMPORTS * STUB_STRIDE <= COMMAND_LINE_OFFSET`
            // is a compile-time assertion).
            unsafe {
                core::ptr::copy_nonoverlapping(
                    stub.as_ptr(),
                    window.add(index * STUB_STRIDE as usize),
                    stub.len(),
                )
            };
        }
        // The path bytes plus an explicit NUL terminator; the frame itself is
        // `int3`-filled, so the terminator must be written, not assumed.
        // SAFETY: `command_line.len() + 1 <= COMMAND_LINE_CAPACITY` was
        // checked above and the fixed region lies inside this one frame.
        unsafe {
            core::ptr::copy_nonoverlapping(
                command_line.as_ptr(),
                window.add(COMMAND_LINE_OFFSET),
                command_line.len(),
            );
            core::ptr::write_volatile(window.add(COMMAND_LINE_OFFSET + command_line.len()), 0);
        }

        if space
            .map_page(WIN32_STUB_VA, frame.address(), USER_CODE)
            .is_err()
        {
            let _ = memory::free_pages(frame, 1);
            rollback(space, &mapped[..mapped_count]);
            return Err(Reject::MapFailed);
        }
        stub_page = WIN32_STUB_VA;

        for (slot, import) in image.imports[..image.import_count].iter().enumerate() {
            let target = loader::CODE_BASE + u64::from(import.iat_rva);
            let Some((physical, _)) = space.translate(target) else {
                unmap_stub_page(space, frame);
                rollback(space, &mapped[..mapped_count]);
                return Err(Reject::ImportTarget);
            };
            let Some(cell) = hhdm.checked_add(physical) else {
                unmap_stub_page(space, frame);
                rollback(space, &mapped[..mapped_count]);
                return Err(Reject::MapFailed);
            };
            let fixed = WIN32_STUB_VA + u64::from(stub_index[slot]) * STUB_STRIDE;
            let cell = cell as *mut u64;
            // SAFETY: `inspect` proved the 8-byte slot lies in-page inside a
            // readable section of the code window; the page is mapped and its
            // frame is reachable through the HHDM. The IAT lives in read-only
            // `.rdata` (R-014), so — like relocations — the fixup is written
            // kernel-side and the mapping flags are unchanged.
            unsafe { core::ptr::write_volatile(cell, fixed) };
            vm::flush(target);
        }
    }

    Ok(Loaded {
        image: *image,
        stub_page,
        applied_relocations: image.relocation_count as u32,
    })
}

/// Unmap the import stub page and free its frame after a post-mapping
/// failure. Best-effort, like [`rollback`].
fn unmap_stub_page(space: &mut AddressSpace, frame: PhysFrame) {
    let _ = space.unmap_page(WIN32_STUB_VA);
    let _ = memory::free_pages(frame, 1);
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
