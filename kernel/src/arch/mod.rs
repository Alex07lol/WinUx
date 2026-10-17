pub mod x86_64;

/// Disable interrupts and halt forever. Safe to call from panic/exception
/// paths: it performs no allocation and takes no locks.
pub fn halt_forever() -> ! {
    x86_64::interrupts_disable();
    loop {
        x86_64::halt();
    }
}
