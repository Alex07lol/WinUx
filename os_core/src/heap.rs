//! Kernel heap: address-ordered free-list allocator with boundary tags.
//!
//! This is the first kernel object allocator (Phase 4). It is deliberately a
//! simple, auditable design rather than a slab/size-class allocator:
//!
//! * The heap owns one contiguous byte region. Every block starts with a
//!   [`BlockHeader`] (span + magic + flags) so a block can be found from its
//!   block start, which is what makes the validating walk easy.
//! * A used block also stores a [`LinkHeader`] immediately before the payload;
//!   it records the block start so `dealloc` can recover the block from the
//!   user pointer even when alignment padding was inserted in front.
//! * Free blocks are kept in a singly linked list sorted by address, with the
//!   link stored in the block payload. Address ordering makes coalescing of
//!   adjacent free blocks a single list walk.
//! * Allocation is first-fit with tail splitting. Alignment padding (for
//!   `Layout::align() > 16`) is absorbed into the allocated block, so every
//!   pointer handed out is `layout.align()`-aligned without leftover fragments
//!   smaller than a block.
//! * Debug behaviour: header magic validation, double-free detection, free
//!   poisoning (`0x55`), and a full [`FreeListHeap::validate`] walk used by
//!   tests and the in-kernel heap self-test.
//!
//! The module performs raw pointer writes into a caller-provided region, so it
//! lives in `os_core` but is exercised by host tests over a leaked `Vec<u8>`
//! and by the kernel over a HHDM-mapped, frame-allocator-owned region.

use core::alloc::Layout;

/// Alignment guaranteed for every block header and payload.
pub const HEAP_ALIGN: usize = 16;
/// Bytes of the block header in front of every block.
pub const BLOCK_HEADER_SIZE: usize = 16;
/// Bytes of the link header immediately before a used payload.
pub const LINK_HEADER_SIZE: usize = 16;
/// Header bytes in front of every used payload (block + link header).
pub const PREFIX_SIZE: usize = BLOCK_HEADER_SIZE + LINK_HEADER_SIZE;
/// Offset of the free-list link inside a free block: behind the link header,
/// so freeing a block never destroys the link header that double-free
/// detection relies on.
const FREE_LINK_OFFSET: usize = PREFIX_SIZE;
/// Smallest block that can exist (headers plus a 16-byte free-list link).
pub const MIN_BLOCK: usize = PREFIX_SIZE + HEAP_ALIGN;

const BLOCK_MAGIC: u32 = 0xB10C_A11C;
const LINK_MAGIC: u32 = 0x11FE_CAFE;
const FLAG_FREE: u32 = 1;

const POISON: u8 = 0x55;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlockHeader {
    /// Total block span in bytes, including the header.
    size: usize,
    magic: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinkHeader {
    /// Address of the block header for this allocation.
    block: usize,
    magic: u32,
    reserved: u32,
}

/// Why [`FreeListHeap::validate`] rejected the heap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeapError {
    /// A header magic was overwritten (buffer underflow or header corruption).
    BadMagic { address: usize },
    /// A block size was zero, unaligned, too small or ran outside the region.
    BadSize { address: usize, size: usize },
    /// A block marked free was not present in the free list (or vice versa).
    FreeListMismatch { address: usize },
    /// Free list addresses are not strictly increasing.
    UnsortedFreeList { address: usize },
    /// A free list entry pointed outside the region.
    FreeListOutOfRange { address: usize },
    /// The block chain did not end exactly at the region end.
    BadCoverage { address: usize },
}

/// Heap accounting computed by a full walk. `errors` counts detected corrupt
/// operations (currently double frees) since initialisation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapStats {
    pub region_bytes: usize,
    pub block_count: usize,
    pub free_blocks: usize,
    pub allocated_blocks: usize,
    pub free_bytes: usize,
    pub allocated_bytes: usize,
    pub largest_free_block: usize,
    pub double_free_detections: usize,
}

impl HeapStats {
    /// Bytes lost to headers plus alignment/splitting remainders.
    pub fn overhead_bytes(&self) -> usize {
        self.region_bytes
            .saturating_sub(self.free_bytes + self.allocated_bytes)
    }
}

fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    value
        .checked_add(align - 1)
        .map(|value| value & !(align - 1))
}

/// First-fit free-list heap over a contiguous region.
///
/// The struct stores the region start/length as integers so it can live in a
/// `static` and be re-initialised without lifetime plumbing. All methods that
/// touch memory are `unsafe` because they write through raw addresses; the
/// invariants are enforced by the constructor plus [`validate`](Self::validate).
pub struct FreeListHeap {
    start: usize,
    len: usize,
    free_head: usize,
    poison: bool,
    double_frees: usize,
    initialised: bool,
}

impl FreeListHeap {
    /// An uninitialised placeholder; [`init`](Self::init) installs a region.
    pub const fn empty() -> Self {
        Self {
            start: 0,
            len: 0,
            free_head: 0,
            poison: true,
            double_frees: 0,
            initialised: false,
        }
    }

    /// Install a heap region. The region must be at least [`MIN_BLOCK`] bytes
    /// and start on a [`HEAP_ALIGN`] boundary; returns `None` otherwise. The
    /// region is trimmed to a whole number of alignment units.
    ///
    /// # Safety
    ///
    /// `region` must be exclusively owned by the heap for the rest of the
    /// program's lifetime: nothing else may read or write those bytes.
    pub unsafe fn init(region: &'static mut [u8]) -> Option<Self> {
        let start = region.as_mut_ptr() as usize;
        let len = region.len() & !(HEAP_ALIGN - 1);
        if len < MIN_BLOCK || !start.is_multiple_of(HEAP_ALIGN) {
            return None;
        }
        let mut heap = Self {
            start,
            len,
            free_head: start,
            poison: true,
            double_frees: 0,
            initialised: true,
        };
        // SAFETY: the whole region is owned by this heap and large enough.
        unsafe {
            heap.write_block_header(
                start,
                BlockHeader {
                    size: len,
                    magic: BLOCK_MAGIC,
                    flags: FLAG_FREE,
                },
            );
            heap.set_next(start, 0);
        }
        Some(heap)
    }

    pub fn is_initialised(&self) -> bool {
        self.initialised
    }

    pub fn region_start(&self) -> usize {
        self.start
    }

    pub fn region_len(&self) -> usize {
        self.len
    }

    /// Enable or disable payload poisoning (0x55 on free and on allocation).
    /// Poisoning is a debug aid; benchmarks disable it to measure the raw
    /// algorithm.
    pub fn set_poison(&mut self, poison: bool) {
        self.poison = poison;
    }

    fn end(&self) -> usize {
        self.start + self.len
    }

    fn in_region(&self, address: usize, size: usize) -> bool {
        address >= self.start
            && address
                .checked_add(size)
                .is_some_and(|end| end <= self.end())
    }

    fn contains(&self, address: usize) -> bool {
        address >= self.start && address < self.end()
    }

    // SAFETY for all raw accessors: callers pass addresses that were produced
    // by walking block headers inside the owned region (validated by
    // `in_region`/`contains`), so the reads/writes stay inside the region.
    // Every such address is a multiple of `HEAP_ALIGN`, which callers check.
    unsafe fn read_block_header(&self, address: usize) -> BlockHeader {
        debug_assert!(self.in_region(address, BLOCK_HEADER_SIZE));
        debug_assert!(address.is_multiple_of(HEAP_ALIGN));
        // SAFETY: guaranteed by the caller's invariant above.
        unsafe { core::ptr::read((address as *const u8).cast::<BlockHeader>()) }
    }

    unsafe fn write_block_header(&mut self, address: usize, header: BlockHeader) {
        debug_assert!(self.in_region(address, BLOCK_HEADER_SIZE));
        debug_assert!(address.is_multiple_of(HEAP_ALIGN));
        // SAFETY: guaranteed by the caller's invariant above.
        unsafe { core::ptr::write((address as *mut u8).cast::<BlockHeader>(), header) };
    }

    unsafe fn next_of(&self, address: usize) -> usize {
        debug_assert!(self.in_region(address, FREE_LINK_OFFSET + 8));
        // SAFETY: every free block is at least MIN_BLOCK bytes, leaving room
        // for the link at FREE_LINK_OFFSET.
        unsafe { core::ptr::read((address + FREE_LINK_OFFSET) as *const usize) }
    }

    unsafe fn set_next(&mut self, address: usize, next: usize) {
        debug_assert!(self.in_region(address, FREE_LINK_OFFSET + 8));
        // SAFETY: as above.
        unsafe { core::ptr::write((address + FREE_LINK_OFFSET) as *mut usize, next) };
    }

    /// Allocate `layout.size()` bytes with `layout.align()` alignment.
    ///
    /// Returns null on exhaustion or when the layout cannot be satisfied.
    pub fn alloc(&mut self, layout: Layout) -> *mut u8 {
        if !self.initialised {
            return core::ptr::null_mut();
        }
        let raw_size = layout.size().max(1);
        let Some(size) = align_up(raw_size, HEAP_ALIGN) else {
            return core::ptr::null_mut();
        };
        let align = layout.align().max(HEAP_ALIGN);
        if size > self.len {
            return core::ptr::null_mut();
        }

        // SAFETY: all addresses below come from the block walk inside the
        // owned region; `fit` re-validates the computed payload position.
        unsafe {
            let mut prev = 0usize;
            let mut current = self.free_head;
            while current != 0 {
                if !self.in_region(current, BLOCK_HEADER_SIZE)
                    || !current.is_multiple_of(HEAP_ALIGN)
                {
                    // Corrupted list: stop rather than write outside the region.
                    return core::ptr::null_mut();
                }
                let header = self.read_block_header(current);
                if header.magic != BLOCK_MAGIC || header.flags & FLAG_FREE == 0 {
                    return core::ptr::null_mut();
                }
                let block_size = header.size;
                if !self.in_region(current, block_size)
                    || block_size < MIN_BLOCK
                    || !block_size.is_multiple_of(HEAP_ALIGN)
                {
                    return core::ptr::null_mut();
                }
                let block_end = current + block_size;
                let next = self.next_of(current);

                if let Some(payload) = Self::fit(current, block_end, size, align) {
                    // The used block spans from the free block start (padding
                    // stays inside the used block, keeping boundary tags valid).
                    let mut span = (payload - current) + size;
                    debug_assert!((payload - current) >= PREFIX_SIZE);

                    // Tail remainder after the allocated block. A remainder too
                    // small to be a block (including a zero-length one) is
                    // absorbed into the allocation so the block walk stays
                    // exact.
                    let alloc_end = current + span;
                    let tail = block_end - alloc_end;
                    let mut link_next = next;
                    if tail >= MIN_BLOCK {
                        self.write_block_header(
                            alloc_end,
                            BlockHeader {
                                size: tail,
                                magic: BLOCK_MAGIC,
                                flags: FLAG_FREE,
                            },
                        );
                        self.set_next(alloc_end, next);
                        link_next = alloc_end;
                    } else {
                        span = block_size;
                    }

                    // Unlink the free node: the allocation replaces it here.
                    if prev == 0 {
                        self.free_head = link_next;
                    } else {
                        self.set_next(prev, link_next);
                    }

                    // Mark used and write the link header in front of the
                    // payload so `dealloc` can recover the block start.
                    self.write_block_header(
                        current,
                        BlockHeader {
                            size: span,
                            magic: BLOCK_MAGIC,
                            flags: 0,
                        },
                    );
                    core::ptr::write(
                        (payload - LINK_HEADER_SIZE) as *mut LinkHeader,
                        LinkHeader {
                            block: current,
                            magic: LINK_MAGIC,
                            reserved: 0,
                        },
                    );
                    if self.poison {
                        self.poison(payload, size);
                    }
                    return payload as *mut u8;
                }

                prev = current;
                current = next;
            }
        }
        core::ptr::null_mut()
    }

    /// Choose the payload position for a used block inside `[block_start,
    /// block_end)` such that the payload is `align`-aligned, the link header
    /// fits after the block header, and the block fits inside the free block.
    fn fit(block_start: usize, block_end: usize, size: usize, align: usize) -> Option<usize> {
        debug_assert!(block_start.is_multiple_of(HEAP_ALIGN));
        let minimum = block_start.checked_add(PREFIX_SIZE)?;
        let payload = align_up(minimum, align)?;
        let end = payload.checked_add(size)?;
        if end <= block_end {
            Some(payload)
        } else {
            None
        }
    }

    /// Free a pointer returned by [`alloc`](Self::alloc).
    ///
    /// Double frees and pointers that were never allocated are detected (via
    /// the magic values and free flag) and ignored; they must never corrupt the
    /// heap.
    ///
    /// # Safety
    ///
    /// `pointer` must be null or a pointer previously returned by this heap.
    pub unsafe fn dealloc(&mut self, pointer: *mut u8) {
        if pointer.is_null() || !self.initialised {
            return;
        }
        let payload = pointer as usize;
        if !payload.is_multiple_of(HEAP_ALIGN) {
            return;
        }
        let link_address = match payload.checked_sub(LINK_HEADER_SIZE) {
            Some(address) => address,
            None => return,
        };
        if !self.contains(link_address) || !self.in_region(link_address, LINK_HEADER_SIZE) {
            return;
        }
        // SAFETY: link_address is inside the region and 16-byte aligned.
        let link = unsafe { core::ptr::read((link_address as *const u8).cast::<LinkHeader>()) };
        if link.magic != LINK_MAGIC || !self.contains(link.block) {
            return;
        }
        let block = link.block;
        if !block.is_multiple_of(HEAP_ALIGN) || !self.in_region(block, BLOCK_HEADER_SIZE) {
            return;
        }
        // SAFETY: block header address is inside the region.
        let header = unsafe { self.read_block_header(block) };
        if header.magic != BLOCK_MAGIC {
            return;
        }
        if header.flags & FLAG_FREE != 0 {
            self.double_frees += 1;
            return;
        }
        let size = header.size;
        if !self.in_region(block, size) || size < MIN_BLOCK {
            return;
        }
        if self.poison {
            // SAFETY: the payload lies inside the owned region.
            unsafe { self.poison(payload, size - (payload - block)) };
        }
        // SAFETY: the block is valid and currently allocated.
        unsafe {
            let header = BlockHeader {
                size,
                magic: BLOCK_MAGIC,
                flags: FLAG_FREE,
            };
            self.write_block_header(block, header);
            self.insert_free(block, size);
        }
    }

    /// Insert a block into the address-ordered free list, coalescing with its
    /// immediate neighbours.
    ///
    /// # Safety
    ///
    /// `address`/`size` must describe a valid block inside the region whose
    /// header has already been rewritten as free.
    unsafe fn insert_free(&mut self, address: usize, size: usize) {
        // SAFETY: caller guarantees the block; list nodes are read within the
        // region. All writes target validated block headers.
        unsafe {
            let mut size = size;
            let mut previous = 0usize;
            let mut current = self.free_head;
            while current != 0 && current < address {
                previous = current;
                current = self.next_of(current);
            }

            // Coalesce with the following block.
            if current != 0 && address + size == current {
                size += self.read_block_header(current).size;
                let after = self.next_of(current);
                self.write_block_header(
                    address,
                    BlockHeader {
                        size,
                        magic: BLOCK_MAGIC,
                        flags: FLAG_FREE,
                    },
                );
                self.set_next(address, after);
            } else {
                self.write_block_header(
                    address,
                    BlockHeader {
                        size,
                        magic: BLOCK_MAGIC,
                        flags: FLAG_FREE,
                    },
                );
                self.set_next(address, current);
            }

            // Coalesce with the preceding block.
            if previous != 0 {
                let previous_size = self.read_block_header(previous).size;
                if previous + previous_size == address {
                    let merged = previous_size + size;
                    let after = self.next_of(address);
                    self.write_block_header(
                        previous,
                        BlockHeader {
                            size: merged,
                            magic: BLOCK_MAGIC,
                            flags: FLAG_FREE,
                        },
                    );
                    self.set_next(previous, after);
                    return;
                }
                self.set_next(previous, address);
            } else {
                self.free_head = address;
            }
        }
    }

    /// Returns the allocated size (including headers and padding) of a live
    /// allocation.
    ///
    /// # Safety
    ///
    /// `pointer` must be a pointer previously returned by this heap.
    pub unsafe fn allocation_size(&self, pointer: *mut u8) -> Option<usize> {
        let payload = pointer as usize;
        if !payload.is_multiple_of(HEAP_ALIGN) {
            return None;
        }
        let link_address = payload.checked_sub(LINK_HEADER_SIZE)?;
        if !self.contains(link_address) || !self.in_region(link_address, LINK_HEADER_SIZE) {
            return None;
        }
        // SAFETY: inside the owned region, 16-byte aligned.
        let link = unsafe { core::ptr::read((link_address as *const u8).cast::<LinkHeader>()) };
        if link.magic != LINK_MAGIC || !self.contains(link.block) {
            return None;
        }
        // SAFETY: inside the owned region.
        let header = unsafe { self.read_block_header(link.block) };
        if header.magic != BLOCK_MAGIC {
            return None;
        }
        Some(header.size)
    }

    // SAFETY: the caller passes a range inside the owned region.
    unsafe fn poison(&mut self, address: usize, length: usize) {
        // SAFETY: guaranteed by the caller.
        unsafe {
            core::ptr::write_bytes(address as *mut u8, POISON, length);
        }
    }

    /// Walk every block and the free list and check the allocator invariants.
    pub fn validate(&self) -> Result<HeapStats, HeapError> {
        let mut stats = HeapStats {
            region_bytes: self.len,
            double_free_detections: self.double_frees,
            ..HeapStats::default()
        };
        if !self.initialised {
            return Ok(stats);
        }

        // SAFETY: the walk stays inside the owned region and only reads.
        unsafe {
            let mut address = self.start;
            while address < self.end() {
                if !self.in_region(address, BLOCK_HEADER_SIZE) {
                    return Err(HeapError::BadCoverage { address });
                }
                let header = self.read_block_header(address);
                if header.magic != BLOCK_MAGIC {
                    return Err(HeapError::BadMagic { address });
                }
                if header.size < MIN_BLOCK
                    || !header.size.is_multiple_of(HEAP_ALIGN)
                    || !self.in_region(address, header.size)
                {
                    return Err(HeapError::BadSize {
                        address,
                        size: header.size,
                    });
                }
                stats.block_count += 1;
                if header.flags & FLAG_FREE != 0 {
                    stats.free_blocks += 1;
                    stats.free_bytes += header.size;
                    stats.largest_free_block = stats.largest_free_block.max(header.size);
                } else {
                    stats.allocated_blocks += 1;
                    stats.allocated_bytes += header.size;
                }
                address += header.size;
            }
            if address != self.end() {
                return Err(HeapError::BadCoverage { address });
            }
            if stats.block_count > self.len / MIN_BLOCK {
                return Err(HeapError::BadCoverage { address });
            }

            // Verify the free list matches the free blocks exactly. The list is
            // short and this runs only from tests/self-tests, so the quadratic
            // membership check is acceptable (and keeps the module allocation-
            // free for `no_std`).
            let mut previous = 0usize;
            let mut node = self.free_head;
            let mut listed = 0usize;
            while node != 0 {
                if !self.contains(node) || !self.in_region(node, FREE_LINK_OFFSET + 8) {
                    return Err(HeapError::FreeListOutOfRange { address: node });
                }
                if previous != 0 && node <= previous {
                    return Err(HeapError::UnsortedFreeList { address: node });
                }
                let header = self.read_block_header(node);
                if header.magic != BLOCK_MAGIC || header.flags & FLAG_FREE == 0 {
                    return Err(HeapError::FreeListMismatch { address: node });
                }
                listed += 1;
                if listed > stats.block_count {
                    return Err(HeapError::FreeListMismatch { address: node });
                }
                previous = node;
                node = self.next_of(node);
            }
            if listed != stats.free_blocks {
                return Err(HeapError::FreeListMismatch {
                    address: self.free_head,
                });
            }
            let mut address = self.start;
            while address < self.end() {
                let header = self.read_block_header(address);
                if header.flags & FLAG_FREE != 0 {
                    let mut occurrences = 0usize;
                    let mut entry = self.free_head;
                    while entry != 0 {
                        if entry == address {
                            occurrences += 1;
                        }
                        entry = self.next_of(entry);
                    }
                    if occurrences != 1 {
                        return Err(HeapError::FreeListMismatch { address });
                    }
                }
                address += header.size;
            }
        }
        Ok(stats)
    }

    /// Heap accounting via a validating walk. Returns `Err` on corruption.
    pub fn stats(&self) -> Result<HeapStats, HeapError> {
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::rng::XorShift64;
    use std::vec::Vec;

    /// Leak a zeroed region so the allocator (and the tests) can treat it as
    /// `'static`, then hand the region to the heap exclusively.
    fn make_heap(bytes: usize) -> FreeListHeap {
        let mut region = std::vec![0u8; bytes];
        let slice: &'static mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(region.as_mut_ptr(), region.len()) };
        std::mem::forget(region);
        unsafe { FreeListHeap::init(slice) }.expect("heap init")
    }

    fn write_pattern(pointer: *mut u8, length: usize, seed: u8) {
        // SAFETY: tests only pass pointers returned by this heap and lengths
        // within the requested allocation.
        unsafe {
            for index in 0..length {
                pointer.add(index).write(seed.wrapping_add(index as u8));
            }
        }
    }

    fn check_pattern(pointer: *const u8, length: usize, seed: u8) {
        // SAFETY: as above.
        unsafe {
            for index in 0..length {
                assert_eq!(pointer.add(index).read(), seed.wrapping_add(index as u8));
            }
        }
    }

    #[test]
    fn initialises_and_has_one_free_block() {
        let heap = make_heap(4096);
        let stats = heap.validate().unwrap();
        assert_eq!(stats.block_count, 1);
        assert_eq!(stats.free_blocks, 1);
        assert_eq!(stats.free_bytes, 4096);
        assert_eq!(stats.allocated_bytes, 0);
    }

    #[test]
    fn basic_alloc_free_round_trip() {
        let mut heap = make_heap(4096);
        let layout = Layout::from_size_align(64, 8).unwrap();
        let pointer = heap.alloc(layout);
        assert!(!pointer.is_null());
        write_pattern(pointer, 64, 0x21);
        check_pattern(pointer, 64, 0x21);
        let stats = heap.validate().unwrap();
        assert_eq!(stats.allocated_blocks, 1);
        assert_eq!(stats.allocated_bytes, PREFIX_SIZE + 64);
        // SAFETY: pointer came from this heap and was not freed yet.
        unsafe { heap.dealloc(pointer) };
        let stats = heap.validate().unwrap();
        assert_eq!(stats.allocated_blocks, 0);
        assert_eq!(stats.free_bytes, 4096);
        assert_eq!(stats.overhead_bytes(), 0);
    }

    #[test]
    fn respects_alignment() {
        let mut heap = make_heap(64 * 1024);
        for align in [1usize, 2, 4, 8, 16, 32, 64, 128, 4096] {
            let layout = Layout::from_size_align(24, align).unwrap();
            let pointer = heap.alloc(layout);
            assert!(!pointer.is_null(), "allocation failed for align {align}");
            assert_eq!(
                pointer as usize % align,
                0,
                "alignment {align} not honoured"
            );
            write_pattern(pointer, 24, align as u8);
        }
        // Freeing in allocation order exercises coalescing with padding.
        let stats = heap.validate().unwrap();
        assert!(stats.allocated_blocks >= 9);
    }

    #[test]
    fn adjacent_blocks_coalesce_on_free() {
        let mut heap = make_heap(4096);
        let layout = Layout::from_size_align(64, 16).unwrap();
        let a = heap.alloc(layout);
        let b = heap.alloc(layout);
        let c = heap.alloc(layout);
        // SAFETY: all pointers came from this heap.
        unsafe {
            heap.dealloc(c);
            heap.dealloc(b);
            heap.dealloc(a);
        }
        let stats = heap.validate().unwrap();
        assert_eq!(stats.free_blocks, 1);
        assert_eq!(stats.free_bytes, 4096);
    }

    #[test]
    fn splits_remainder_and_reuses_it() {
        let mut heap = make_heap(4096);
        let big = Layout::from_size_align(256, 16).unwrap();
        let small = Layout::from_size_align(32, 16).unwrap();
        // SAFETY: pointers come from this heap.
        unsafe {
            let a = heap.alloc(big);
            let b = heap.alloc(small);
            let c = heap.alloc(small);
            assert!(!a.is_null() && !b.is_null() && !c.is_null());
            heap.dealloc(a);
            // The freed 256-byte block must be reusable for `small`.
            let d = heap.alloc(small);
            assert_eq!(d, a);
            heap.dealloc(d);
            heap.dealloc(b);
            heap.dealloc(c);
        }
        assert_eq!(heap.validate().unwrap().free_blocks, 1);
    }

    #[test]
    fn detects_double_free_without_corruption() {
        let mut heap = make_heap(4096);
        let layout = Layout::from_size_align(64, 16).unwrap();
        let pointer = heap.alloc(layout);
        // SAFETY: double free is the case under test.
        unsafe {
            heap.dealloc(pointer);
            heap.dealloc(pointer);
        }
        let stats = heap.validate().unwrap();
        assert_eq!(stats.double_free_detections, 1);
        assert_eq!(stats.free_blocks, 1);
        assert_eq!(stats.free_bytes, 4096);
    }

    #[test]
    fn ignores_foreign_and_misaligned_pointers() {
        let mut heap = make_heap(4096);
        let mut stack = [0u8; 16];
        // SAFETY: expected to be rejected by the heap.
        unsafe {
            heap.dealloc(stack.as_mut_ptr());
            let pointer = heap.alloc(Layout::from_size_align(32, 16).unwrap());
            heap.dealloc(pointer.add(1));
            heap.dealloc(pointer);
        }
        assert!(heap.validate().is_ok());
    }

    #[test]
    fn exhaustion_returns_null_and_heap_recovers() {
        let mut heap = make_heap(1024);
        let layout = Layout::from_size_align(256, 16).unwrap();
        let mut pointers = Vec::new();
        for _ in 0..3 {
            pointers.push(heap.alloc(layout));
        }
        assert!(pointers.iter().all(|pointer| !pointer.is_null()));
        assert!(heap.alloc(layout).is_null(), "expected exhaustion");
        // SAFETY: pointers came from this heap.
        unsafe {
            for pointer in pointers {
                heap.dealloc(pointer);
            }
        }
        assert_eq!(heap.validate().unwrap().free_bytes, 1024);
        let pointer = heap.alloc(layout);
        assert!(!pointer.is_null());
        // SAFETY: as above.
        unsafe { heap.dealloc(pointer) };
    }

    #[test]
    fn allocation_larger_than_region_fails() {
        let mut heap = make_heap(1024);
        let layout = Layout::from_size_align(4096, 16).unwrap();
        assert!(heap.alloc(layout).is_null());
    }

    #[test]
    fn random_churn_preserves_contents_and_accounting() {
        let mut heap = make_heap(256 * 1024);
        let mut rng = XorShift64::new(0xDEAD_BEEF);
        let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
        for step in 0..20_000u64 {
            let action = rng.next_usize(100);
            if action < 55 || live.is_empty() {
                let size = 1 + rng.next_usize(1024);
                let align = 1usize << rng.next_usize(7); // 1..64
                let layout = Layout::from_size_align(size, align).unwrap();
                let pointer = heap.alloc(layout);
                if !pointer.is_null() {
                    let seed = (step & 0xff) as u8;
                    write_pattern(pointer, size, seed);
                    live.push((pointer, size, seed));
                }
            } else {
                let index = rng.next_usize(live.len());
                let (pointer, size, seed) = live.swap_remove(index);
                check_pattern(pointer, size, seed);
                // SAFETY: pointer is live and came from this heap.
                unsafe { heap.dealloc(pointer) };
            }
        }
        // Release the rest, verifying every block on the way out.
        for (pointer, size, seed) in live {
            check_pattern(pointer, size, seed);
            // SAFETY: pointer is live and came from this heap.
            unsafe { heap.dealloc(pointer) };
        }
        let stats = heap.validate().unwrap();
        assert_eq!(stats.allocated_blocks, 0);
        assert_eq!(stats.free_bytes, 256 * 1024);
        assert_eq!(stats.double_free_detections, 0);
    }

    #[test]
    fn fragmentation_is_reported() {
        let mut heap = make_heap(64 * 1024);
        let mut rng = XorShift64::new(7);
        let mut live = Vec::new();
        for _ in 0..64 {
            let size = 16 + rng.next_usize(512);
            let pointer = heap.alloc(Layout::from_size_align(size, 16).unwrap());
            assert!(!pointer.is_null());
            live.push((pointer, size));
        }
        // Free every other block to create alternating holes.
        for index in (0..live.len()).step_by(2) {
            // SAFETY: live pointer from this heap.
            unsafe { heap.dealloc(live[index].0) };
            live[index].0 = core::ptr::null_mut();
        }
        let stats = heap.validate().unwrap();
        assert!(stats.free_blocks > 1, "expected checkered free list");
        assert!(stats.largest_free_block < stats.free_bytes);
        for (pointer, _) in live {
            if !pointer.is_null() {
                // SAFETY: live pointer from this heap.
                unsafe { heap.dealloc(pointer) };
            }
        }
        assert_eq!(heap.validate().unwrap().free_blocks, 1);
    }

    #[test]
    fn all_allocations_are_16_byte_aligned() {
        let mut heap = make_heap(8192);
        let mut rng = XorShift64::new(99);
        let mut pointers = Vec::new();
        for _ in 0..15 {
            let size = 1 + rng.next_usize(200);
            let pointer = heap.alloc(Layout::from_size_align(size, 1).unwrap());
            assert!(!pointer.is_null(), "allocation failed");
            assert_eq!(pointer as usize % HEAP_ALIGN, 0);
            pointers.push(pointer);
        }
        assert!(heap.validate().is_ok());
        // SAFETY: pointers came from this heap.
        unsafe {
            for pointer in pointers {
                heap.dealloc(pointer);
            }
        }
        assert_eq!(heap.validate().unwrap().free_bytes, 8192);
    }

    #[test]
    fn allocation_size_is_reported() {
        let mut heap = make_heap(4096);
        let pointer = heap.alloc(Layout::from_size_align(100, 8).unwrap());
        assert!(!pointer.is_null());
        // SAFETY: pointer came from this heap.
        let size = unsafe { heap.allocation_size(pointer) }.unwrap();
        assert!(size >= PREFIX_SIZE + 100);
        assert!(size.is_multiple_of(HEAP_ALIGN));
        // SAFETY: as above.
        unsafe { heap.dealloc(pointer) };
    }

    /// Small-region churn that validates the whole heap after every single
    /// operation, so any boundary-tag mistake fails on the exact step that
    /// caused it. This caught a real absorbed-tail bug during development.
    #[test]
    fn churn_validates_after_every_step() {
        let mut heap = make_heap(4096);
        let mut rng = XorShift64::new(0xDEAD_BEEF);
        let mut live: Vec<(*mut u8, usize, u8)> = Vec::new();
        for step in 0..500u64 {
            let action = rng.next_usize(100);
            if action < 55 || live.is_empty() {
                let size = 1 + rng.next_usize(1024);
                let align = 1usize << rng.next_usize(7);
                let layout = Layout::from_size_align(size, align).unwrap();
                let pointer = heap.alloc(layout);
                if !pointer.is_null() {
                    let seed = (step & 0xff) as u8;
                    write_pattern(pointer, size, seed);
                    live.push((pointer, size, seed));
                }
            } else {
                let index = rng.next_usize(live.len());
                let (pointer, size, seed) = live.swap_remove(index);
                check_pattern(pointer, size, seed);
                // SAFETY: live pointer from this heap.
                unsafe { heap.dealloc(pointer) };
            }
            if let Err(error) = heap.validate() {
                panic!("corruption after step {step}: {error:?}");
            }
        }
    }
}
