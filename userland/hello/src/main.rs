#![no_std]
#![no_main]

use project_os_user_runtime::{entry, sys_write};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    entry(bin_main)
}

fn bin_main() {
    sys_write(1, b"user: hello from ring 3 via int 0x80\n");
}
