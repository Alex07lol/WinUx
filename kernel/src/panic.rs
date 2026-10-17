use core::panic::PanicInfo;

use crate::serial;

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    serial::force_write_str("\r\nPROJECT_OS_PANIC: ");
    serial::force_print(format_args!("{}", info.message()));
    if let Some(location) = info.location() {
        serial::force_print(format_args!(" at {}:{}", location.file(), location.line()));
    }
    serial::force_write_str("\r\n");
    crate::arch::halt_forever();
}
