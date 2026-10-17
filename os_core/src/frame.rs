//! Physical frame allocator (bitmap, first-fit with a rotating cursor).
//!
//! Design notes:
//! * One bit per 4 KiB frame. A set bit means *in use*; the allocator starts
//!   with every frame in use and only explicitly usable RAM is released to it.
//!   Bootloader-reclaimable, ACPI, framebuffer and kernel regions therefore
//!   cannot be handed out by accident.
//! * Allocation supports power-of-two contiguous runs and returns naturally
//!   aligned frames (a run of `n` frames is `n * 4 KiB` aligned). This is the
//!   shape a buddy allocator would use, without claiming to be one yet.
//! * `free_pages` rejects unaligned, out-of-range and double frees. The kernel
//!   keeps this allocator bit-exact with the host tests.

pub const PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysFrame {
    base: u64,
}

impl PhysFrame {
    /// Construct a frame handle from a page-aligned physical address.
    pub const fn from_address(address: u64) -> Option<Self> {
        if !address.is_multiple_of(PAGE_SIZE) {
            None
        } else {
            Some(Self { base: address })
        }
    }

    pub const fn address(self) -> u64 {
        self.base
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    UnalignedRegion { base: u64, length: u64 },
    RangeOverflow,
    OutOfRange { frame: u64 },
    UnalignedFree { frame: u64, count: usize },
    InvalidCount(usize),
    NotAllocated { frame: u64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    pub total_frames: usize,
    pub free_frames: usize,
    pub largest_free_run: usize,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Bitmap-backed allocator borrowing caller-owned storage.
pub struct FrameAllocator<'a> {
    bits: &'a mut [u64],
    frame_limit: usize,
    free_frames: usize,
    cursor: usize,
}

impl<'a> FrameAllocator<'a> {
    /// Create an allocator covering at most `bits.len() * 64` frames and never
    /// more than `max_physical_address / PAGE_SIZE`. All frames start in use.
    pub fn new(bits: &'a mut [u64], max_physical_address: u64) -> Self {
        for word in bits.iter_mut() {
            *word = u64::MAX;
        }
        let capacity = bits.len().saturating_mul(64);
        let by_address = usize::try_from(max_physical_address / PAGE_SIZE).unwrap_or(usize::MAX);
        let frame_limit = capacity.min(by_address);
        Self {
            bits,
            frame_limit,
            free_frames: 0,
            cursor: 0,
        }
    }

    #[inline]
    fn is_used(&self, frame: usize) -> bool {
        self.bits[frame / 64] & (1u64 << (frame % 64)) != 0
    }

    #[inline]
    fn set_used(&mut self, frame: usize) {
        self.bits[frame / 64] |= 1u64 << (frame % 64);
    }

    #[inline]
    fn set_free(&mut self, frame: usize) {
        self.bits[frame / 64] &= !(1u64 << (frame % 64));
    }

    /// Release a page-aligned usable range to the allocator. Returns how many
    /// frames became free. Calling this twice for the same range is harmless.
    pub fn add_usable_region(&mut self, base: u64, length: u64) -> Result<usize, FrameError> {
        if !base.is_multiple_of(PAGE_SIZE) || !length.is_multiple_of(PAGE_SIZE) {
            return Err(FrameError::UnalignedRegion { base, length });
        }
        let end = base.checked_add(length).ok_or(FrameError::RangeOverflow)?;
        let start = usize::try_from(base / PAGE_SIZE).unwrap_or(usize::MAX);
        if start >= self.frame_limit {
            return Ok(0);
        }
        let end = usize::try_from(end / PAGE_SIZE)
            .unwrap_or(usize::MAX)
            .min(self.frame_limit);
        if end <= start {
            return Ok(0);
        }

        let mut freed = 0usize;
        for frame in start..end {
            if self.is_used(frame) {
                self.set_free(frame);
                freed += 1;
            }
        }
        self.free_frames += freed;
        Ok(freed)
    }

    /// Mark a physical range as reserved. Idempotent; ranges outside the
    /// managed window are clamped. Used for low memory and future MMIO holes.
    pub fn reserve_range(&mut self, base: u64, length: u64) -> usize {
        if length == 0 {
            return 0;
        }
        let start = usize::try_from(base / PAGE_SIZE).unwrap_or(usize::MAX);
        let end = base.saturating_add(length).div_ceil(PAGE_SIZE);
        let end = usize::try_from(end)
            .unwrap_or(usize::MAX)
            .min(self.frame_limit);
        if start >= self.frame_limit || end <= start {
            return 0;
        }

        let mut taken = 0usize;
        for frame in start..end {
            if !self.is_used(frame) {
                self.set_used(frame);
                self.free_frames = self.free_frames.saturating_sub(1);
                taken += 1;
            }
        }
        taken
    }

    /// Allocate `count` contiguous frames. `count` must be a power of two and
    /// the returned block is aligned to `count * 4 KiB`.
    pub fn alloc_pages(&mut self, count: usize) -> Option<PhysFrame> {
        if count == 0 || !count.is_power_of_two() || count > self.frame_limit {
            return None;
        }

        let starts = self.frame_limit.div_ceil(count);
        let mut start = self.cursor & !(count - 1);
        if start >= self.frame_limit {
            start = 0;
        }

        for _ in 0..starts {
            if start
                .checked_add(count)
                .is_none_or(|end| end > self.frame_limit)
            {
                start = 0;
            }
            if self.run_is_free(start, count) {
                let end = start + count;
                for frame in start..end {
                    self.set_used(frame);
                }
                self.free_frames -= count;
                self.cursor = if end >= self.frame_limit { 0 } else { end };
                return PhysFrame::from_address((start as u64) * PAGE_SIZE);
            }
            start += count;
            if start >= self.frame_limit {
                start = 0;
            }
        }
        None
    }

    pub fn alloc(&mut self) -> Option<PhysFrame> {
        self.alloc_pages(1)
    }

    fn run_is_free(&self, start: usize, count: usize) -> bool {
        let Some(end) = start.checked_add(count) else {
            return false;
        };
        if end > self.frame_limit {
            return false;
        }
        // Check whole words where possible, then the ragged edges bitwise.
        let mut frame = start;
        while frame < end && !frame.is_multiple_of(64) {
            if self.is_used(frame) {
                return false;
            }
            frame += 1;
        }
        while frame + 64 <= end {
            if self.bits[frame / 64] != 0 {
                return false;
            }
            frame += 64;
        }
        while frame < end {
            if self.is_used(frame) {
                return false;
            }
            frame += 1;
        }
        true
    }

    /// Release a previously allocated contiguous block. Rejects unaligned
    /// blocks, blocks that cross the managed window and double frees.
    pub fn free_pages(&mut self, frame: PhysFrame, count: usize) -> Result<(), FrameError> {
        if count == 0 || !count.is_power_of_two() {
            return Err(FrameError::InvalidCount(count));
        }
        if count > self.frame_limit {
            return Err(FrameError::OutOfRange {
                frame: frame.address(),
            });
        }
        let start_u64 = frame.address() / PAGE_SIZE;
        let span = (count as u64)
            .checked_mul(PAGE_SIZE)
            .ok_or(FrameError::OutOfRange {
                frame: frame.address(),
            })?;
        if !frame.address().is_multiple_of(span) {
            return Err(FrameError::UnalignedFree {
                frame: frame.address(),
                count,
            });
        }
        let start = usize::try_from(start_u64).map_err(|_| FrameError::OutOfRange {
            frame: frame.address(),
        })?;
        let Some(end) = start.checked_add(count) else {
            return Err(FrameError::OutOfRange {
                frame: frame.address(),
            });
        };
        if end > self.frame_limit {
            return Err(FrameError::OutOfRange {
                frame: frame.address(),
            });
        }
        for index in start..end {
            if !self.is_used(index) {
                return Err(FrameError::NotAllocated {
                    frame: (index as u64) * PAGE_SIZE,
                });
            }
        }
        for index in start..end {
            self.set_free(index);
        }
        self.free_frames += count;
        Ok(())
    }

    pub fn free(&mut self, frame: PhysFrame) -> Result<(), FrameError> {
        self.free_pages(frame, 1)
    }

    pub fn stats(&self) -> FrameStats {
        let mut run = 0usize;
        let mut largest = 0usize;
        for frame in 0..self.frame_limit {
            if self.is_used(frame) {
                run = 0;
            } else {
                run += 1;
                largest = largest.max(run);
            }
        }
        FrameStats {
            total_frames: self.frame_limit,
            free_frames: self.free_frames,
            largest_free_run: largest,
            total_bytes: (self.frame_limit as u64) * PAGE_SIZE,
            free_bytes: (self.free_frames as u64) * PAGE_SIZE,
        }
    }

    pub fn frame_limit(&self) -> usize {
        self.frame_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn storage(max_address: u64) -> Vec<u64> {
        let frames = (max_address / PAGE_SIZE) as usize;
        vec![0u64; frames.div_ceil(64)]
    }

    #[test]
    fn starts_with_everything_reserved() {
        let mut bits = storage(MIB);
        let mut allocator = FrameAllocator::new(&mut bits, MIB);
        assert_eq!(allocator.stats().total_frames, 256);
        assert_eq!(allocator.stats().free_frames, 0);
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn releases_only_explicitly_usable_ranges() {
        let mut bits = storage(MIB);
        let mut allocator = FrameAllocator::new(&mut bits, MIB);
        assert_eq!(allocator.add_usable_region(0, 16 * PAGE_SIZE), Ok(16));
        assert_eq!(allocator.stats().free_frames, 16);
        assert_eq!(allocator.add_usable_region(0, 16 * PAGE_SIZE), Ok(0));
        assert_eq!(allocator.stats().free_frames, 16);
    }

    #[test]
    fn rejects_unaligned_regions_and_overflow() {
        let mut bits = storage(MIB);
        let mut allocator = FrameAllocator::new(&mut bits, MIB);
        assert!(matches!(
            allocator.add_usable_region(1, PAGE_SIZE),
            Err(FrameError::UnalignedRegion { .. })
        ));
        assert_eq!(
            allocator.add_usable_region(u64::MAX - 0xfff, PAGE_SIZE),
            Err(FrameError::RangeOverflow)
        );
    }

    #[test]
    fn allocations_are_aligned_and_distinct() {
        let mut bits = storage(64 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut bits, 64 * PAGE_SIZE);
        allocator.add_usable_region(0, 64 * PAGE_SIZE).unwrap();
        let frames: Vec<_> = (0..8).filter_map(|_| allocator.alloc()).collect();
        assert_eq!(frames.len(), 8);
        for frame in &frames {
            assert_eq!(frame.address() % PAGE_SIZE, 0);
        }
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn reserves_low_memory_and_never_hands_it_out() {
        let mut bits = storage(4 * MIB);
        let mut allocator = FrameAllocator::new(&mut bits, 4 * MIB);
        allocator.add_usable_region(0, 4 * MIB).unwrap();
        assert_eq!(allocator.reserve_range(0, MIB), 256);
        assert_eq!(allocator.stats().free_frames, 3 * 256);
        for _ in 0..(3 * 256) {
            let frame = allocator.alloc().unwrap();
            assert!(frame.address() >= MIB);
        }
        assert_eq!(allocator.alloc(), None);
    }

    #[test]
    fn contiguous_allocation_is_aligned_to_its_size() {
        let mut bits = storage(64 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut bits, 64 * PAGE_SIZE);
        allocator.add_usable_region(0, 64 * PAGE_SIZE).unwrap();
        allocator.reserve_range(0, PAGE_SIZE);
        let block = allocator.alloc_pages(8).unwrap();
        assert_eq!(block.address() % (8 * PAGE_SIZE), 0);
        assert_eq!(block.address(), 8 * PAGE_SIZE);
        let next = allocator.alloc_pages(4).unwrap();
        assert_eq!(next.address() % (4 * PAGE_SIZE), 0);
    }

    #[test]
    fn exhaustion_returns_none_and_recovers_after_free() {
        let mut bits = storage(4 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut bits, 4 * PAGE_SIZE);
        allocator.add_usable_region(0, 4 * PAGE_SIZE).unwrap();
        let frames: Vec<_> = (0..4).filter_map(|_| allocator.alloc()).collect();
        assert_eq!(frames.len(), 4);
        assert_eq!(allocator.alloc(), None);
        allocator.free(frames[2]).unwrap();
        assert!(allocator.alloc().is_some());
        assert_eq!(allocator.stats().free_frames, 0);
    }

    #[test]
    fn detects_double_free_and_foreign_free() {
        let mut bits = storage(8 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut bits, 8 * PAGE_SIZE);
        allocator.add_usable_region(0, 8 * PAGE_SIZE).unwrap();
        let first = allocator.alloc().unwrap();
        allocator.free(first).unwrap();
        assert_eq!(
            allocator.free(first),
            Err(FrameError::NotAllocated {
                frame: first.address()
            })
        );
        let second = allocator.alloc().unwrap();
        assert_eq!(allocator.stats().free_frames, 7);
        // Freeing a frame that was never allocated (the last page) must not
        // corrupt accounting.
        let never_allocated = PhysFrame::from_address(7 * PAGE_SIZE).unwrap();
        assert_eq!(
            allocator.free(never_allocated),
            Err(FrameError::NotAllocated {
                frame: never_allocated.address()
            })
        );
        assert_eq!(allocator.stats().free_frames, 7);
        allocator.free(second).unwrap();
        assert_eq!(allocator.stats().free_frames, 8);
    }

    #[test]
    fn rejects_bad_free_requests() {
        let mut bits = storage(8 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut bits, 8 * PAGE_SIZE);
        allocator.add_usable_region(0, 8 * PAGE_SIZE).unwrap();
        allocator.reserve_range(0, PAGE_SIZE);
        let frame = allocator.alloc().unwrap();
        assert_eq!(frame.address(), PAGE_SIZE);
        assert!(matches!(
            allocator.free_pages(frame, 4),
            Err(FrameError::UnalignedFree { .. })
        ));
        assert_eq!(
            allocator.free_pages(frame, 3),
            Err(FrameError::InvalidCount(3))
        );
        assert_eq!(
            allocator.free_pages(PhysFrame::from_address(0x1_0000).unwrap(), 1),
            Err(FrameError::OutOfRange { frame: 0x1_0000 })
        );
        // A power-of-two count larger than the managed window must be rejected
        // before any alignment multiplication, so absurd values cannot overflow.
        assert_eq!(
            allocator.free_pages(frame, usize::MAX / 2 + 1),
            Err(FrameError::OutOfRange {
                frame: frame.address()
            })
        );
    }

    #[test]
    fn clamps_to_physical_limit_and_bitmap_capacity() {
        // Bitmap is large enough for 1 MiB, but the platform only has 64 KiB.
        let mut bits = storage(MIB);
        let mut allocator = FrameAllocator::new(&mut bits, 64 * 1024);
        assert_eq!(allocator.frame_limit(), 16);
        assert_eq!(allocator.add_usable_region(0, MIB), Ok(16));
        assert_eq!(allocator.stats().free_frames, 16);

        // Bitmap smaller than the physical limit: capacity wins. A one-word
        // bitmap covers 64 frames regardless of the 64 MiB address limit.
        let mut small = storage(16 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut small, 64 * MIB);
        assert_eq!(allocator.frame_limit(), 64);
        assert_eq!(allocator.add_usable_region(0, 64 * MIB), Ok(64));
    }

    #[test]
    fn free_churn_does_not_leak() {
        let mut bits = storage(2 * MIB);
        let mut allocator = FrameAllocator::new(&mut bits, 2 * MIB);
        allocator.add_usable_region(0x10_0000, MIB).unwrap();
        allocator.reserve_range(0, 0x10_0000);
        let baseline = allocator.stats().free_frames;
        let mut rng = crate::rng::XorShift64::new(0x5eed_1234);
        let mut live = Vec::new();
        for _ in 0..20_000 {
            if live.is_empty() || rng.next_bool() {
                if let Some(frame) = allocator.alloc() {
                    live.push(frame);
                }
            } else {
                let index = rng.next_usize(live.len());
                let frame = live.swap_remove(index);
                allocator.free(frame).unwrap();
            }
        }
        for frame in live {
            allocator.free(frame).unwrap();
        }
        assert_eq!(allocator.stats().free_frames, baseline);
        assert_eq!(
            allocator
                .alloc_pages(256)
                .map(|f| f.address() % (256 * PAGE_SIZE)),
            Some(0)
        );
    }

    #[test]
    fn reports_largest_free_run() {
        let mut bits = storage(16 * PAGE_SIZE);
        let mut allocator = FrameAllocator::new(&mut bits, 16 * PAGE_SIZE);
        allocator.add_usable_region(0, 16 * PAGE_SIZE).unwrap();
        allocator.alloc_pages(4).unwrap();
        let middle = allocator.alloc_pages(2).unwrap();
        assert_eq!(allocator.stats().largest_free_run, 10);
        allocator.free_pages(middle, 2).unwrap();
        assert_eq!(allocator.stats().largest_free_run, 12);
    }
}
