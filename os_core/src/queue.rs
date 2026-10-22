//! Bounded single-producer/single-consumer queue (Phase 9 stage 7).
//!
//! The interrupt handler is the only producer and the network service path is
//! the only consumer. Slots are published with release stores and observed
//! with acquire loads, so no lock is taken on either side and a handler can
//! never deadlock against a lock held by the interrupted code.
//!
//! The queue stores `Copy` values only. It is intentionally not a general
//! channel: `reset` is only safe while both sides are quiescent, and a full
//! queue returns the rejected value instead of blocking or allocating.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A fixed-capacity FIFO for one producer and one consumer.
///
/// `N` must be greater than zero. `head` counts published pushes, `tail`
/// counts completed pops, and the slot index is the counter modulo `N`. The
/// free-running counters make a full queue distinguishable from an empty one
/// without sacrificing a slot.
pub struct SpscQueue<T: Copy, const N: usize> {
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: the producer only writes the slot at its private `head`, then
// publishes it; the consumer only reads a slot after observing the matching
// head and releases it through `tail`. `reset` requires exclusive access.
unsafe impl<T: Copy + Send, const N: usize> Sync for SpscQueue<T, N> {}
// SAFETY: the queue owns its slots and moves only `T` values between them.
unsafe impl<T: Copy + Send, const N: usize> Send for SpscQueue<T, N> {}

impl<T: Copy, const N: usize> SpscQueue<T, N> {
    pub const fn new() -> Self {
        assert!(N > 0, "SpscQueue capacity must be non-zero");
        Self {
            slots: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    /// Number of published values not yet popped. The result is a snapshot;
    /// it is exact for a single-threaded caller and advisory otherwise.
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail).min(N)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() == N
    }

    /// Append `value`, returning it unchanged when the queue is full.
    /// Must only be called by the single producer.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= N {
            return Err(value);
        }
        let index = head % N;
        // SAFETY: only the producer writes this slot, and the consumer cannot
        // observe it until `head` is published below.
        unsafe {
            (*self.slots[index].get()).write(value);
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Remove and return the oldest value, or `None` when empty.
    /// Must only be called by the single consumer.
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        if head == tail {
            return None;
        }
        let index = tail % N;
        // SAFETY: the consumer only reads a slot whose push has been observed
        // through `head`; the producer will not reuse it until `tail` advances.
        let value = unsafe { (*self.slots[index].get()).assume_init() };
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Discard all queued values. The caller must guarantee that no producer
    /// or consumer can run concurrently (for example while the line is
    /// masked); values are `Copy`, so no destructor is skipped.
    pub fn reset(&self) {
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
    }
}

impl<T: Copy, const N: usize> Default for SpscQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::SpscQueue;

    #[test]
    fn preserves_order_and_reports_empty() {
        let queue = SpscQueue::<u32, 4>::new();
        assert!(queue.is_empty());
        assert!(!queue.is_full());
        for value in 0..4 {
            assert!(queue.try_push(value).is_ok());
        }
        assert!(queue.is_full());
        assert_eq!(queue.len(), 4);
        for value in 0..4 {
            assert_eq!(queue.try_pop(), Some(value));
        }
        assert_eq!(queue.try_pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn full_queue_returns_the_rejected_value() {
        let queue = SpscQueue::<u32, 2>::new();
        assert!(queue.try_push(10).is_ok());
        assert!(queue.try_push(11).is_ok());
        assert_eq!(queue.try_push(12), Err(12));
        assert_eq!(queue.try_pop(), Some(10));
        assert!(queue.try_push(12).is_ok());
        assert_eq!(queue.try_pop(), Some(11));
        assert_eq!(queue.try_pop(), Some(12));
    }

    #[test]
    fn wraps_across_slots_many_times() {
        let queue = SpscQueue::<usize, 3>::new();
        for round in 0..100 {
            assert!(queue.try_push(round).is_ok());
            assert!(queue.try_push(round + 1000).is_ok());
            assert_eq!(queue.try_pop(), Some(round));
            assert_eq!(queue.try_pop(), Some(round + 1000));
        }
        assert!(queue.is_empty());
    }

    #[test]
    fn reset_discards_queued_values() {
        let queue = SpscQueue::<u8, 4>::new();
        assert!(queue.try_push(1).is_ok());
        assert!(queue.try_push(2).is_ok());
        queue.reset();
        assert!(queue.is_empty());
        assert_eq!(queue.try_pop(), None);
        assert!(queue.try_push(3).is_ok());
        assert_eq!(queue.try_pop(), Some(3));
    }

    #[test]
    fn producer_and_consumer_threads_see_every_value_in_order() {
        const ITEMS: usize = 20_000;
        let queue = SpscQueue::<usize, 8>::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for value in 0..ITEMS {
                    let mut pending = value;
                    loop {
                        match queue.try_push(pending) {
                            Ok(()) => break,
                            Err(rejected) => {
                                pending = rejected;
                                core::hint::spin_loop();
                            }
                        }
                    }
                }
            });
            scope.spawn(|| {
                for expected in 0..ITEMS {
                    loop {
                        if let Some(value) = queue.try_pop() {
                            assert_eq!(value, expected);
                            break;
                        }
                        core::hint::spin_loop();
                    }
                }
            });
        });
        assert!(queue.is_empty());
    }
}
