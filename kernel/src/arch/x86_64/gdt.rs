//! Global Descriptor Table, 64-bit TSS and a double-fault IST stack.
//!
//! Limine supplies a usable GDT, but the kernel must own its descriptor tables
//! before installing an IDT with an IST-backed double-fault handler. Selectors:
//! 0x08 kernel code, 0x10 kernel data, 0x18 TSS (two slots), 0x28 user data
//! (DPL3 → 0x2b), 0x30 user code (DPL3 → 0x33).

use core::arch::asm;
use core::cell::UnsafeCell;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Tss {
    reserved0: u32,
    pub rsp0: u64,
    pub rsp1: u64,
    pub rsp2: u64,
    reserved1: u64,
    pub ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    pub iomap_base: u16,
}

impl Tss {
    const fn zeroed() -> Self {
        Self {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: 0,
        }
    }
}

#[repr(C, align(16))]
struct IstStack([u8; 16 * 1024]);

static DOUBLE_FAULT_STACK: IstStack = IstStack([0; 16 * 1024]);

struct SyncCell<T>(UnsafeCell<T>);
// SAFETY: descriptor-table initialisation is single-threaded and happens
// before interrupts are enabled.
unsafe impl<T> Sync for SyncCell<T> {}

static TSS: SyncCell<Tss> = SyncCell(UnsafeCell::new(Tss::zeroed()));
static GDT: SyncCell<[u64; 7]> = SyncCell(UnsafeCell::new([0; 7]));

/// User data selector (DPL3).
pub const USER_DATA_SELECTOR: u16 = 0x28 | 3;
/// User code selector (DPL3).
pub const USER_CODE_SELECTOR: u16 = 0x30 | 3;

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

fn tss() -> &'static mut Tss {
    // SAFETY: single-threaded initialisation; the TSS is never aliased while
    // the CPU uses it because only this module mutates it, once.
    unsafe { &mut *TSS.0.get() }
}

fn gdt() -> &'static mut [u64; 7] {
    // SAFETY: as above.
    unsafe { &mut *GDT.0.get() }
}

/// Point the TSS `rsp0` at the kernel stack that ring-3 entries (interrupts,
/// exceptions and `int 0x80` coming from CPL3) must use. Called before every
/// switch into user mode with the current thread's kernel stack top.
pub fn set_tss_rsp0(value: u64) {
    tss().rsp0 = value & !0xf;
}

fn make_tss_descriptor(base: u64, limit: u32) -> [u64; 2] {
    let mut low = 0u64;
    low |= (limit as u64) & 0xffff;
    low |= (base & 0x00ff_ffff) << 16;
    low |= 0x89u64 << 40; // present, 64-bit available TSS
    low |= ((limit as u64 >> 16) & 0x0f) << 48;
    low |= ((base >> 24) & 0xff) << 56;
    let high = base >> 32;
    [low, high]
}

/// Build and load our GDT/TSS. Must be called before `idt::init`.
pub fn init() {
    let kernel_stack: u64;
    // SAFETY: reading rsp is side-effect free.
    unsafe { asm!("mov {}, rsp", out(reg) kernel_stack, options(nomem, nostack)) };

    let tss_ref = tss();
    // rsp0 is only used on a ring transition. User mode does not exist yet, so
    // the current kernel stack is a safe placeholder and is documented as such.
    tss_ref.rsp0 = kernel_stack & !0xf;
    tss_ref.ist[0] = {
        let stack = &raw const DOUBLE_FAULT_STACK as u64;
        (stack + core::mem::size_of::<IstStack>() as u64) & !0xf
    };
    tss_ref.iomap_base = core::mem::size_of::<Tss>() as u16;

    let entries = gdt();
    entries[0] = 0;
    // 64-bit kernel code: base 0, limit 4 GiB granularity, L=1, present, DPL0.
    entries[1] = 0x00af_9a00_0000_ffff;
    // 64-bit kernel data: present, writable, DPL0.
    entries[2] = 0x00af_9200_0000_ffff;
    let tss_base = &raw const TSS as u64;
    let [low, high] = make_tss_descriptor(tss_base, (core::mem::size_of::<Tss>() - 1) as u32);
    entries[3] = low;
    entries[4] = high;
    // 64-bit user data (0x28|3 = 0x2b): present, writable, DPL3.
    entries[5] = 0x00af_f200_0000_ffff;
    // 64-bit user code (0x30|3 = 0x33): present, readable, DPL3.
    entries[6] = 0x00af_fa00_0000_ffff;

    let pointer = DescriptorTablePointer {
        limit: (core::mem::size_of::<[u64; 7]>() - 1) as u16,
        base: entries.as_ptr() as u64,
    };

    // SAFETY: the GDT memory is static and valid for the kernel lifetime. The
    // far return reloads CS with the new code selector; all data selectors and
    // the task register are then reloaded before interrupts are enabled.
    unsafe {
        asm!("lgdt [{}]", in(reg) &pointer, options(readonly, nostack, preserves_flags));
        asm!(
            "push 0x08",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            out("rax") _,
            options(preserves_flags)
        );
        asm!(
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "mov fs, ax",
            "mov gs, ax",
            out("ax") _,
            options(nomem, nostack, preserves_flags)
        );
        asm!("mov ax, 0x18", "ltr ax", out("ax") _, options(nomem, nostack, preserves_flags));
    }
}
