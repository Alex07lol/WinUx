#![no_std]
#![no_main]

use project_os_user_runtime::entry;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    entry(bin_main)
}

fn bin_main() {
    // SAFETY: the address is deliberately unmapped in the user address space.
    // The resulting page fault is the point of the program; the kernel must
    // isolate it and keep running.
    unsafe { core::ptr::write_volatile(0x5_0000_0000 as *mut u64, 0xdead) };
}
