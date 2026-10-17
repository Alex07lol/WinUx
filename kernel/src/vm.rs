//! Kernel-owned 4-level address spaces (Phase 3 completion).
//!
//! Until this milestone the kernel ran on page tables owned by Limine. This
//! module builds the kernel's own PML4, copies the kernel half (PML4 entries
//! 256..512, covering the HHDM and the kernel image) from the live tables so
//! the switch is invisible to running kernel code, and switches CR3 onto the
//! new tables. From here on every address space the kernel creates is built
//! from the kernel template written at boot.
//!
//! Design decisions:
//!
//! * The user half (PML4 entries 0..256) is private to each address space and
//!   is what [`AddressSpace::destroy`] tears down.
//! * The kernel half is shared by pointing at the same lower-level tables as
//!   the template. Only user-half mappings are ever edited here, so no kernel
//!   table is mutated through another address space.
//! * W^X is enforced at the API boundary: every mapping request that is
//!   writable must set `NO_EXECUTE` (see `os_core::paging::is_wx_safe`).
//! * The bootloader's page tables and the frames they reference are left
//!   allocated but unused after the switch. Reclaiming them safely needs arena
//!   ownership tracking (bootloader-reclaimable ranges) that arrives with the
//!   scheduler-era memory work; leaking them until then is the honest choice.
//!
//! All entry access goes through the HHDM, which the copied kernel half keeps
//! mapped. TLB maintenance is one `invlpg` per changed page (single CPU).

use core::arch::asm;
use core::sync::atomic::{Ordering, fence};

use os_core::frame::PhysFrame;
use os_core::paging as page_math;
use os_core::paging::{PRESENT, USER, USER_CODE, USER_DATA, WRITABLE};

use crate::memory;
use crate::sync::SpinLock;

const CR3_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const ENTRY_COUNT: usize = 512;
const KERNEL_HALF_START: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VmError {
    OutOfMemory,
    AddressOverflow,
    NotAligned,
    NotCanonical,
    /// A writable, executable mapping was requested (violates W^X).
    WxViolation,
    /// A user-accessible page was requested in the kernel half.
    UserPageInKernelHalf,
    /// The request did not set the present bit.
    MissingPresent,
    /// An intermediate entry is a huge page; splitting is not implemented.
    LargePage,
    AlreadyMapped,
    NotMapped,
}

fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: CR3 is readable in ring 0.
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn write_cr3(value: u64) {
    // SAFETY: loading CR3 with a page-aligned PML4 physical address is valid
    // in ring 0; the new hierarchy must (and does) keep the current
    // instruction stream and stack mapped, which the copied kernel half
    // guarantees.
    unsafe { asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags)) };
}

fn flush(virtual_address: u64) {
    // SAFETY: `invlpg` is valid in ring 0 for any address; it only drops a
    // TLB entry.
    unsafe {
        asm!(
            "invlpg [{}]",
            in(reg) virtual_address,
            options(nostack, preserves_flags)
        )
    };
    fence(Ordering::SeqCst);
}

/// One 4-level address space (PML4 + everything reachable through its user
/// half).
pub struct AddressSpace {
    pml4_physical: u64,
    hhdm: u64,
}

impl AddressSpace {
    /// Create a space whose kernel half is a copy of the live (template)
    /// tables' kernel half. The user half starts empty.
    pub fn new_like_kernel(hhdm_offset: u64) -> Result<Self, VmError> {
        let live = read_cr3() & CR3_ADDRESS_MASK;
        let space = Self::new_bare(hhdm_offset)?;
        let templates = space.table(live)?;
        let target = space.table(space.pml4_physical)?;
        for index in KERNEL_HALF_START..ENTRY_COUNT {
            // SAFETY: both are valid 512-entry tables in HHDM-mapped RAM.
            let entry = unsafe { core::ptr::read_volatile(templates.add(index)) };
            // SAFETY: as above.
            unsafe { core::ptr::write_volatile(target.add(index), entry) };
        }
        fence(Ordering::SeqCst);
        Ok(space)
    }

    /// Create a space with an all-zero PML4 (no kernel half). Only useful for
    /// tests; activating it would fault immediately.
    pub fn new_bare(hhdm_offset: u64) -> Result<Self, VmError> {
        let frame = memory::alloc_frame().map_err(|_| VmError::OutOfMemory)?;
        let physical = frame.address();
        let space = Self {
            pml4_physical: physical,
            hhdm: hhdm_offset,
        };
        let table = space.table(physical)?;
        space.zero_table(table);
        Ok(space)
    }

    pub fn pml4_physical(&self) -> u64 {
        self.pml4_physical
    }

    /// Load this address space's PML4 into CR3.
    pub fn activate(&self) -> Result<(), VmError> {
        write_cr3(self.pml4_physical);
        Ok(())
    }

    fn table(&self, physical: u64) -> Result<*mut u64, VmError> {
        let virtual_address = self
            .hhdm
            .checked_add(physical)
            .ok_or(VmError::AddressOverflow)?;
        Ok(virtual_address as *mut u64)
    }

    fn zero_table(&self, table: *mut u64) {
        // SAFETY: every table is a 4 KiB frame in HHDM-mapped RAM.
        unsafe {
            for index in 0..ENTRY_COUNT {
                core::ptr::write_volatile(table.add(index), 0);
            }
        }
    }

    fn allocate_zeroed_table(&self) -> Result<u64, VmError> {
        let frame = memory::alloc_frame().map_err(|_| VmError::OutOfMemory)?;
        let physical = frame.address();
        let table = self.table(physical)?;
        self.zero_table(table);
        Ok(physical)
    }

    fn entry(table: *mut u64, index: usize) -> u64 {
        // SAFETY: callers pass a valid 512-entry table and a masked index.
        unsafe { core::ptr::read_volatile(table.add(index)) }
    }

    fn set_entry(table: *mut u64, index: usize, value: u64) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile(table.add(index), value) };
    }

    fn table_is_empty(&self, physical: u64) -> Result<bool, VmError> {
        let table = self.table(physical)?;
        Ok((0..ENTRY_COUNT).all(|index| Self::entry(table, index) == 0))
    }

    fn free_frame(&self, physical: u64) -> Result<(), VmError> {
        let frame = PhysFrame::from_address(physical).ok_or(VmError::NotAligned)?;
        memory::free_pages(frame, 1).map_err(|_| VmError::NotMapped)
    }

    /// Normalise and validate a mapping request.
    fn validate_mapping(virtual_address: u64, physical: u64, flags: u64) -> Result<(), VmError> {
        if !virtual_address.is_multiple_of(page_math::PAGE_SIZE)
            || !physical.is_multiple_of(page_math::PAGE_SIZE)
        {
            return Err(VmError::NotAligned);
        }
        if !page_math::is_canonical_4level(virtual_address) {
            return Err(VmError::NotCanonical);
        }
        if !page_math::is_wx_safe(flags) {
            return Err(VmError::WxViolation);
        }
        if flags & PRESENT == 0 {
            return Err(VmError::MissingPresent);
        }
        let indices = page_math::indices_4level(virtual_address);
        if flags & USER != 0 && indices.pml4 >= KERNEL_HALF_START {
            return Err(VmError::UserPageInKernelHalf);
        }
        Ok(())
    }

    /// Map one 4 KiB page. Intermediate tables are created on demand, never
    /// split: hitting a huge-page entry is an error.
    pub fn map_page(
        &mut self,
        virtual_address: u64,
        physical: u64,
        flags: u64,
    ) -> Result<(), VmError> {
        Self::validate_mapping(virtual_address, physical, flags)?;
        let indices = page_math::indices_4level(virtual_address);
        let intermediate_flags = PRESENT | WRITABLE | (flags & USER);

        let mut current = self.pml4_physical;
        for index in [indices.pml4, indices.pdpt, indices.pd] {
            let table = self.table(current)?;
            let entry = Self::entry(table, index);
            if page_math::entry_present(entry) {
                if page_math::entry_huge(entry) {
                    return Err(VmError::LargePage);
                }
                current = page_math::entry_address(entry);
            } else {
                let fresh = self.allocate_zeroed_table()?;
                let new_entry = page_math::make_entry(fresh, intermediate_flags)
                    .ok_or(VmError::AddressOverflow)?;
                Self::set_entry(table, index, new_entry);
                fence(Ordering::SeqCst);
                current = fresh;
            }
        }

        let pt = self.table(current)?;
        if page_math::entry_present(Self::entry(pt, indices.pt)) {
            return Err(VmError::AlreadyMapped);
        }
        let leaf = page_math::make_entry(physical, flags).ok_or(VmError::AddressOverflow)?;
        Self::set_entry(pt, indices.pt, leaf);
        fence(Ordering::SeqCst);
        flush(virtual_address);
        Ok(())
    }

    /// Map `bytes` starting at `virtual_address` to `physical`, page by page.
    #[allow(dead_code)] // consumed by the ELF/PE loaders (Phase 5b/7).
    pub fn map_region(
        &mut self,
        virtual_address: u64,
        physical: u64,
        bytes: u64,
        flags: u64,
    ) -> Result<(), VmError> {
        if !bytes.is_multiple_of(page_math::PAGE_SIZE) {
            return Err(VmError::NotAligned);
        }
        for offset in (0..bytes).step_by(page_math::PAGE_SIZE as usize) {
            self.map_page(virtual_address + offset, physical + offset, flags)?;
        }
        Ok(())
    }

    /// Remove one 4 KiB mapping and return the physical address it pointed
    /// at. Intermediate tables left empty in the user half are freed.
    pub fn unmap_page(&mut self, virtual_address: u64) -> Result<u64, VmError> {
        if !virtual_address.is_multiple_of(page_math::PAGE_SIZE) {
            return Err(VmError::NotAligned);
        }
        if !page_math::is_canonical_4level(virtual_address) {
            return Err(VmError::NotCanonical);
        }
        let indices = page_math::indices_4level(virtual_address);

        let mut levels = [0u64; 3];
        let mut current = self.pml4_physical;
        for (depth, index) in [indices.pml4, indices.pdpt, indices.pd]
            .into_iter()
            .enumerate()
        {
            let table = self.table(current)?;
            let entry = Self::entry(table, index);
            if !page_math::entry_present(entry) {
                return Err(VmError::NotMapped);
            }
            if page_math::entry_huge(entry) {
                return Err(VmError::LargePage);
            }
            levels[depth] = current;
            current = page_math::entry_address(entry);
        }

        let pt = self.table(current)?;
        let leaf = Self::entry(pt, indices.pt);
        if !page_math::entry_present(leaf) {
            return Err(VmError::NotMapped);
        }
        if page_math::entry_huge(leaf) {
            return Err(VmError::LargePage);
        }
        let released = page_math::entry_address(leaf);
        Self::set_entry(pt, indices.pt, 0);
        fence(Ordering::SeqCst);
        flush(virtual_address);

        // Free now-empty user-half tables from the bottom up. Kernel-half
        // tables are shared with the template and are never freed here.
        if indices.pml4 < KERNEL_HALF_START {
            let mut child = current;
            for (depth, index) in [indices.pd, indices.pdpt, indices.pml4]
                .into_iter()
                .enumerate()
            {
                if !self.table_is_empty(child)? {
                    break;
                }
                let parent = levels[2 - depth];
                let parent_table = self.table(parent)?;
                Self::set_entry(parent_table, index, 0);
                fence(Ordering::SeqCst);
                self.free_frame(child)?;
                child = parent;
            }
        }
        Ok(released)
    }

    /// Translate one address, returning the physical base address of the
    /// mapping and the raw entry flags, or `None` if unmapped.
    pub fn translate(&self, virtual_address: u64) -> Option<(u64, u64)> {
        if !page_math::is_canonical_4level(virtual_address) {
            return None;
        }
        let indices = page_math::indices_4level(virtual_address);
        let mut current = self.pml4_physical;
        for (depth, index) in [indices.pml4, indices.pdpt, indices.pd]
            .into_iter()
            .enumerate()
        {
            let table = self.table(current).ok()?;
            let entry = Self::entry(table, index);
            if !page_math::entry_present(entry) {
                return None;
            }
            if page_math::entry_huge(entry) {
                let shift = match depth {
                    1 => 30,
                    2 => 21,
                    // The huge bit is reserved in PML4 entries; treat as absent.
                    _ => return None,
                };
                let mask = (1u64 << shift) - 1;
                let physical = page_math::entry_address(entry) + (virtual_address & mask);
                return Some((physical, entry));
            }
            current = page_math::entry_address(entry);
        }
        let pt = self.table(current).ok()?;
        let leaf = Self::entry(pt, indices.pt);
        if !page_math::entry_present(leaf) {
            return None;
        }
        Some((page_math::entry_address(leaf), leaf))
    }

    /// Tear the space down: free the user half and the PML4. The space must
    /// not be active in CR3, and its user-half pages must already be unmapped
    /// or owned elsewhere.
    #[allow(dead_code)] // consumed by process teardown (Phase 5b).
    pub fn destroy(mut self) -> Result<(), VmError> {
        for pml4_index in 0..KERNEL_HALF_START {
            let pml4 = self.table(self.pml4_physical)?;
            let pdpt_entry = Self::entry(pml4, pml4_index);
            if !page_math::entry_present(pdpt_entry) || page_math::entry_huge(pdpt_entry) {
                continue;
            }
            let pdpt_physical = page_math::entry_address(pdpt_entry);
            let pdpt = self.table(pdpt_physical)?;
            for pdpt_index in 0..ENTRY_COUNT {
                let pd_entry = Self::entry(pdpt, pdpt_index);
                if !page_math::entry_present(pd_entry) || page_math::entry_huge(pd_entry) {
                    continue;
                }
                let pd_physical = page_math::entry_address(pd_entry);
                let pd = self.table(pd_physical)?;
                for pd_index in 0..ENTRY_COUNT {
                    let pt_entry = Self::entry(pd, pd_index);
                    if !page_math::entry_present(pt_entry) || page_math::entry_huge(pt_entry) {
                        continue;
                    }
                    self.free_frame(page_math::entry_address(pt_entry))?;
                }
                self.free_frame(pd_physical)?;
            }
            self.free_frame(pdpt_physical)?;
        }
        self.free_frame(self.pml4_physical)?;
        self.pml4_physical = 0;
        Ok(())
    }
}

static KERNEL_PML4: SpinLock<Option<u64>> = SpinLock::new(None);
static HHDM_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The kernel template PML4 installed by [`init`], used as the kernel half of
/// every future user address space.
pub fn kernel_pml4() -> Option<u64> {
    *KERNEL_PML4.lock()
}

/// The HHDM offset recorded by [`init`], or `None` before boot initialisation.
pub fn hhdm_offset() -> Option<u64> {
    match HHDM_OFFSET.load(Ordering::SeqCst) {
        0 => None,
        offset => Some(offset),
    }
}

/// Switch CR3 back to the kernel template space (after user-mode work).
pub fn activate_kernel() -> Result<(), VmError> {
    let pml4 = kernel_pml4().ok_or(VmError::NotMapped)?;
    let hhdm = hhdm_offset().ok_or(VmError::NotMapped)?;
    let space = AddressSpace {
        pml4_physical: pml4,
        hhdm,
    };
    space.activate()
}

/// Translate an address in whichever address space is active (live CR3).
/// Used by the syscall boundary to validate user pointers.
pub fn translate_live(virtual_address: u64) -> Option<(u64, u64)> {
    let hhdm = hhdm_offset()?;
    let space = AddressSpace {
        pml4_physical: read_cr3() & CR3_ADDRESS_MASK,
        hhdm,
    };
    space.translate(virtual_address)
}

/// Build the kernel's own address space, prove it works, then switch CR3 onto
/// it. Called once during boot with interrupts disabled.
pub fn init(hhdm_offset: u64) -> Result<(), &'static str> {
    if kernel_pml4().is_some() {
        return Err("vm: kernel address space already installed");
    }
    let mut space = AddressSpace::new_like_kernel(hhdm_offset)
        .map_err(|_| "vm: cannot create kernel address space")?;
    self_test(&mut space)?;
    let pml4 = space.pml4_physical();
    space.activate().map_err(|_| "vm: CR3 switch failed")?;
    if read_cr3() & CR3_ADDRESS_MASK != pml4 {
        return Err("vm: CR3 does not hold the kernel PML4");
    }
    HHDM_OFFSET.store(hhdm_offset, Ordering::SeqCst);
    *KERNEL_PML4.lock() = Some(pml4);
    Ok(())
}

/// Mapping, translation, W^X, fault and CR3-switch checks against a private
/// address space. All frames and tables allocated here are released again.
pub fn self_test(space: &mut AddressSpace) -> Result<(), &'static str> {
    const CODE_VA: u64 = 0x40_0000;
    const DATA_VA: u64 = 0x41_0000;
    const SPARE_VA: u64 = 0x42_0000;

    let code_frame = memory::alloc_frame().map_err(|_| "vm selftest: no frame for code")?;
    let data_frame = memory::alloc_frame().map_err(|_| "vm selftest: no frame for data")?;

    // Write a pattern into the code frame through the HHDM, then read it back
    // through the new mapping after activation.
    let code_physical = code_frame.address();
    let code_window = space
        .table(code_physical)
        .map_err(|_| "vm selftest: HHDM window failed")? as *mut u8;
    // SAFETY: the frame was just allocated for this test; the HHDM maps it.
    unsafe {
        for index in 0..64 {
            code_window.add(index).write_volatile(index as u8 ^ 0x5A);
        }
    }

    space
        .map_page(CODE_VA, code_physical, USER_CODE)
        .map_err(|_| "vm selftest: mapping user code failed")?;
    space
        .map_page(DATA_VA, data_frame.address(), USER_DATA)
        .map_err(|_| "vm selftest: mapping user data failed")?;

    match space.translate(CODE_VA) {
        Some((physical, leaf))
            if physical == code_physical && leaf == code_physical | USER_CODE => {}
        _ => return Err("vm selftest: code translation mismatch"),
    }
    if space.translate(SPARE_VA).is_some() {
        return Err("vm selftest: unmapped address translated");
    }

    // W^X: writable without NO_EXECUTE must be refused.
    match space.map_page(SPARE_VA, code_physical, PRESENT | WRITABLE | USER) {
        Err(VmError::WxViolation) => {}
        _ => return Err("vm selftest: W^X violation was not refused"),
    }
    // Double mapping must be refused.
    match space.map_page(CODE_VA, data_frame.address(), USER_DATA) {
        Err(VmError::AlreadyMapped) => {}
        _ => return Err("vm selftest: double mapping was not refused"),
    }
    // Misaligned virtual address must be refused.
    match space.map_page(CODE_VA + 1, data_frame.address(), USER_DATA) {
        Err(VmError::NotAligned) => {}
        _ => return Err("vm selftest: misaligned mapping was not refused"),
    }
    // User page in the kernel half must be refused.
    match space.map_page(0xffff_8000_0000_1000, data_frame.address(), USER_DATA) {
        Err(VmError::UserPageInKernelHalf) => {}
        _ => return Err("vm selftest: user page in kernel half was not refused"),
    }
    // Non-canonical virtual address must be refused.
    match space.map_page(0x0000_8000_0000_1000, data_frame.address(), USER_DATA) {
        Err(VmError::NotCanonical) => {}
        _ => return Err("vm selftest: non-canonical mapping was not refused"),
    }

    // Switch onto the new tables and touch both user pages: the kernel half
    // copy must keep this code running, and the user half must be live.
    let previous_cr3 = read_cr3();
    space
        .activate()
        .map_err(|_| "vm selftest: activation failed")?;
    // SAFETY: DATA_VA is a live read-write user mapping owned by this test.
    unsafe {
        let target = DATA_VA as *mut u64;
        target.write_volatile(0x1122_3344_5566_7788);
        if target.read_volatile() != 0x1122_3344_5566_7788 {
            write_cr3(previous_cr3);
            return Err("vm selftest: user data page write did not survive");
        }
    }
    // SAFETY: CODE_VA is a live read-only user mapping of the code frame.
    unsafe {
        for index in 0..64 {
            if (CODE_VA as *const u8).add(index).read_volatile() != index as u8 ^ 0x5A {
                write_cr3(previous_cr3);
                return Err("vm selftest: code page contents mismatch");
            }
        }
    }
    write_cr3(previous_cr3);

    // Unmap the data page: the physical address must come back and the
    // translation must be gone.
    match space.unmap_page(DATA_VA) {
        Ok(released) if released == data_frame.address() => {}
        _ => return Err("vm selftest: unmap did not return the data frame"),
    }
    if space.translate(DATA_VA).is_some() {
        return Err("vm selftest: unmapped address still translates");
    }
    match space.unmap_page(DATA_VA) {
        Err(VmError::NotMapped) => {}
        _ => return Err("vm selftest: double unmap was not refused"),
    }
    space
        .unmap_page(CODE_VA)
        .map_err(|_| "vm selftest: unmapping code failed")?;

    memory::free_pages(data_frame, 1).map_err(|_| "vm selftest: freeing data frame failed")?;
    memory::free_pages(code_frame, 1).map_err(|_| "vm selftest: freeing code frame failed")?;
    Ok(())
}
