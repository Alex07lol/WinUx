//! Kernel synchronization primitives (single-CPU bring-up).
//!
//! * [`SpinLock`] is a plain spin lock for state that is only touched with
//!   interrupts disabled or from code that cannot be preempted.
//! * [`IrqSpinLock`] additionally disables interrupts for the critical
//!   section. On a single CPU this is what makes a lock safe against the timer
//!   preemption path: a lock held by a preempted thread would otherwise never
//!   be released, because interrupt handlers never block.
//!
//! SMP would need real per-CPU handling (and, for [`SpinLock`], a review of
//! every use); that is recorded in docs/ARCHITECTURE.md.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::x86_64::{interrupts_restore, interrupts_save_disable};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialised by the atomic `locked` flag.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard { lock: self }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock for its lifetime.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock for its lifetime.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// Spin lock that disables interrupts for the critical section.
///
/// The saved RFLAGS are kept in the guard, so nested locks on the same CPU
/// restore the correct state even when the outer section already had
/// interrupts disabled (for example when called from an interrupt handler).
pub struct IrqSpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: access to `data` is serialised by the atomic `locked` flag, and the
// guard also excludes interrupt handlers on the same CPU.
unsafe impl<T: Send> Sync for IrqSpinLock<T> {}
unsafe impl<T: Send> Send for IrqSpinLock<T> {}

impl<T> IrqSpinLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> IrqSpinLockGuard<'_, T> {
        let flags = interrupts_save_disable();
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        IrqSpinLockGuard { lock: self, flags }
    }

    /// Attempt the lock without spinning. Returns `None` when the lock is held
    /// (including by the caller itself), restoring interrupts immediately.
    /// Used for best-effort work that must never deadlock, such as mirroring
    /// panic output to the screen while the console lock may already be held.
    pub fn try_lock(&self) -> Option<IrqSpinLockGuard<'_, T>> {
        let flags = interrupts_save_disable();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            interrupts_restore(flags);
            return None;
        }
        Some(IrqSpinLockGuard { lock: self, flags })
    }
}

pub struct IrqSpinLockGuard<'a, T> {
    lock: &'a IrqSpinLock<T>,
    flags: u64,
}

impl<T> Deref for IrqSpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the guard holds the lock for its lifetime.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for IrqSpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard holds the lock for its lifetime.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for IrqSpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        interrupts_restore(self.flags);
    }
}
