//! Kernel heap bring-up and the global allocator (Phase 4).
//!
//! The heap is a fixed 8 MiB region of contiguous frames leased from the frame
//! allocator at boot and accessed through the HHDM (the bootloader maps all
//! usable RAM there). The region is never returned, which is deliberate: this
//! milestone needs a stable home for kernel data structures, not a growable
//! heap. Page-granular growth belongs to the kernel-owned page-table work in
//! [`crate::vm`] (Phase 3 completion).
//!
//! All allocator access is serialised by an [`IrqSpinLock`], so allocating
//! from an interrupt handler is safe on the single CPU: a preempted thread
//! cannot be holding the lock when the handler runs, because the lock disables
//! interrupts for its critical section.

use core::alloc::{GlobalAlloc, Layout};

use os_core::frame::PAGE_SIZE;
use os_core::heap::{FreeListHeap, HeapError, HeapStats};

use crate::memory;
use crate::sync::IrqSpinLock;

/// Fixed heap size for this milestone.
pub const HEAP_BYTES: usize = 8 * 1024 * 1024;
const HEAP_PAGES: usize = HEAP_BYTES / PAGE_SIZE as usize;

/// Global allocator wrapper around [`FreeListHeap`].
pub struct GlobalHeap {
    heap: IrqSpinLock<FreeListHeap>,
}

impl GlobalHeap {
    pub const fn new() -> Self {
        Self {
            heap: IrqSpinLock::new(FreeListHeap::empty()),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.heap.lock().is_initialised()
    }

    pub fn install(&self, region: &'static mut [u8]) -> Result<HeapStats, &'static str> {
        let mut heap = self.heap.lock();
        // SAFETY: `region` is a unique, page-aligned, permanently leased
        // allocation from the frame allocator; nothing else aliases it.
        *heap = unsafe { FreeListHeap::init(region) }.ok_or("heap region rejected")?;
        heap.stats().map_err(|_| "kernel heap stats failed")
    }

    pub fn stats(&self) -> Result<HeapStats, HeapError> {
        self.heap.lock().stats()
    }

    pub fn validate(&self) -> Result<HeapStats, HeapError> {
        self.heap.lock().validate()
    }
}

// SAFETY: `FreeListHeap` is plain `Send` data; every access goes through the
// IRQ-safe lock, which excludes both other threads and interrupt handlers on
// this CPU.
unsafe impl GlobalAlloc for GlobalHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.heap.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        // SAFETY: the `GlobalAlloc::dealloc` contract requires `pointer` to
        // come from a previous `GlobalHeap::alloc` of the same layout, which
        // is exactly what `FreeListHeap::dealloc` expects.
        unsafe { self.heap.lock().dealloc(pointer) };
    }
}

#[global_allocator]
pub static GLOBAL_ALLOCATOR: GlobalHeap = GlobalHeap::new();

/// Lease the heap region from the frame allocator and initialise the heap.
pub fn init(hhdm_offset: u64) -> Result<HeapStats, &'static str> {
    if GLOBAL_ALLOCATOR.is_ready() {
        return Err("kernel heap already initialised");
    }
    let frames = memory::alloc_pages(HEAP_PAGES)?;
    let physical = frames.address();
    let base = hhdm_offset
        .checked_add(physical)
        .ok_or("kernel heap HHDM address overflowed")?;
    // SAFETY: the frames were exclusively allocated for the heap and the HHDM
    // maps every usable RAM range, including this one. The borrow is
    // permanent: the region is never returned to the frame allocator, so no
    // other kernel code can alias it.
    let region: &'static mut [u8] =
        unsafe { core::slice::from_raw_parts_mut(base as *mut u8, HEAP_BYTES) };
    GLOBAL_ALLOCATOR.install(region)
}

/// Exercise the global allocator: alignment, contents, growth, large
/// allocation, deallocation and a full validation walk.
pub fn self_test() -> Result<HeapStats, &'static str> {
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    let baseline = GLOBAL_ALLOCATOR
        .stats()
        .map_err(|_| "kernel heap stats failed")?;

    let boxed = Box::new(0xA5A5_1234_DEAD_BEEFu64);
    if *boxed != 0xA5A5_1234_DEAD_BEEFu64 {
        return Err("boxed value read-back mismatch");
    }
    if !(Box::as_ref(&boxed) as *const u64 as usize).is_multiple_of(8) {
        return Err("boxed allocation is not 8-byte aligned");
    }
    drop(boxed);

    let mut vector: Vec<u32> = Vec::new();
    for index in 0..10_000u32 {
        vector.push(index.wrapping_mul(2_654_435_761));
    }
    for (index, value) in vector.iter().enumerate() {
        if *value != (index as u32).wrapping_mul(2_654_435_761) {
            return Err("vector contents corrupted");
        }
    }
    if vector.capacity() < 10_000 {
        return Err("vector did not grow as expected");
    }
    drop(vector);

    // A large allocation crosses many heap pages and many free blocks.
    let large_layout = Layout::from_size_align(1024 * 1024, 16).map_err(|_| "bad layout")?;
    // SAFETY: the layout is valid; a non-null result owns `size` bytes and is
    // freed below.
    let large = unsafe { GLOBAL_ALLOCATOR.alloc(large_layout) };
    if large.is_null() {
        return Err("1 MiB heap allocation failed");
    }
    // SAFETY: the allocation owns `large_layout.size()` writable bytes.
    unsafe {
        for offset in (0..large_layout.size()).step_by(4096) {
            large.add(offset).write_volatile(offset as u8);
        }
        for offset in (0..large_layout.size()).step_by(4096) {
            if large.add(offset).read_volatile() != offset as u8 {
                return Err("large allocation read-back mismatch");
            }
        }
        GLOBAL_ALLOCATOR.dealloc(large, large_layout);
    }

    let stats = GLOBAL_ALLOCATOR
        .validate()
        .map_err(|_| "kernel heap validation failed")?;
    if stats.free_bytes != baseline.free_bytes {
        return Err("kernel heap accounting mismatch after self-test");
    }
    Ok(stats)
}
