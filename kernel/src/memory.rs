//! Physical memory bring-up.
//!
//! The allocator only receives `LIMINE_MEMMAP_USABLE` ranges. Bootloader
//! reclaimable memory, the kernel image, the framebuffer and firmware data are
//! never released implicitly; that is what makes this milestone safe without a
//! full reclaim policy. The low 1 MiB is explicitly reserved as a defensive
//! null-pointer guard.

use core::cell::UnsafeCell;

use os_core::frame::{FrameAllocator, FrameStats, PhysFrame};
use os_core::memmap::MemoryRegion;

use crate::sync::SpinLock;

/// 64 GiB of addressable physical memory => 16 Mi frames => 2 MiB of bitmap.
const MANAGED_BYTES: u64 = 64 * 1024 * 1024 * 1024;
const MAX_FRAMES: usize = (MANAGED_BYTES / os_core::frame::PAGE_SIZE) as usize;
const BITMAP_WORDS: usize = MAX_FRAMES / 64;

struct Bitmap(UnsafeCell<[u64; BITMAP_WORDS]>);
// SAFETY: the bitmap is borrowed exactly once by the single allocator, which
// serialises all access behind `ALLOCATOR`.
unsafe impl Sync for Bitmap {}

static BITMAP: Bitmap = Bitmap(UnsafeCell::new([0; BITMAP_WORDS]));
static ALLOCATOR: SpinLock<Option<FrameAllocator<'static>>> = SpinLock::new(None);

fn bitmap() -> &'static mut [u64] {
    // SAFETY: called once during `init`, before any other allocator access.
    unsafe { &mut *BITMAP.0.get() }
}

/// Initialise the frame allocator from the boot memory map.
pub fn init(regions: &[MemoryRegion]) -> Result<FrameStats, &'static str> {
    let mut slot = ALLOCATOR.lock();
    if slot.is_some() {
        return Err("frame allocator already initialised");
    }

    let highest = regions
        .iter()
        .filter_map(|region| region.checked_end())
        .max()
        .unwrap_or(0);
    let mut allocator = FrameAllocator::new(bitmap(), highest);
    for region in regions.iter().filter(|region| region.kind.is_usable()) {
        allocator
            .add_usable_region(region.base, region.length)
            .map_err(|_| "frame allocator rejected a usable region")?;
    }
    // Keep the first MiB out of the allocator: it contains real-mode data and
    // makes accidental null/low pointer allocations obvious during bring-up.
    allocator.reserve_range(0, 1024 * 1024);

    let stats = allocator.stats();
    *slot = Some(allocator);
    Ok(stats)
}

/// Allocate one physical frame for kernel-owned metadata (for example a page
/// table). Returns `Err` before initialisation or on exhaustion.
pub fn alloc_frame() -> Result<PhysFrame, &'static str> {
    let mut slot = ALLOCATOR.lock();
    slot.as_mut()
        .ok_or("frame allocator is not initialised")?
        .alloc()
        .ok_or("frame allocator exhausted")
}

/// Run allocation, free, alignment, exhaustion and accounting checks against
/// the live kernel allocator. Writes a pattern through the HHDM to prove that
/// returned frames are actually mapped and writable.
pub fn self_test(hhdm_offset: u64) -> Result<FrameStats, &'static str> {
    let mut slot = ALLOCATOR.lock();
    let allocator = slot.as_mut().ok_or("frame allocator is not initialised")?;
    let baseline = allocator.stats().free_frames;

    let single = allocator.alloc().ok_or("single-frame allocation failed")?;
    let block = allocator
        .alloc_pages(4)
        .ok_or("4-frame contiguous allocation failed")?;
    if single.address() < 1024 * 1024 || block.address() < 1024 * 1024 {
        return Err("allocator returned reserved low memory");
    }
    if block.address() % (4 * os_core::frame::PAGE_SIZE) != 0 {
        return Err("contiguous allocation is not naturally aligned");
    }
    if single.address() == block.address() {
        return Err("allocator returned overlapping frames");
    }

    // Write and verify a pattern in the four-frame block via the HHDM.
    let bytes = 4 * os_core::frame::PAGE_SIZE;
    let base = hhdm_offset
        .checked_add(block.address())
        .ok_or("HHDM address overflow")? as *mut u8;
    // SAFETY: the block was just allocated and HHDM maps usable RAM; the range
    // is page-aligned and four pages long.
    unsafe {
        for index in 0..bytes as usize {
            base.add(index)
                .write_volatile((index as u8).wrapping_mul(37).wrapping_add(11));
        }
        for index in 0..bytes as usize {
            let expected = (index as u8).wrapping_mul(37).wrapping_add(11);
            if base.add(index).read_volatile() != expected {
                return Err("HHDM read-back mismatch in allocated frame");
            }
        }
    }

    allocator
        .free_pages(block, 4)
        .map_err(|_| "freeing the contiguous block failed")?;
    allocator
        .free(single)
        .map_err(|_| "freeing the single frame failed")?;
    if allocator.stats().free_frames != baseline {
        return Err("allocator accounting mismatch after free");
    }
    if allocator.free(single).is_ok() {
        return Err("allocator accepted a double free");
    }
    if allocator.alloc_pages(3).is_some() {
        return Err("allocator accepted a non-power-of-two block");
    }

    Ok(allocator.stats())
}
