//! Limine boot protocol bindings (protocol base revision 6).
//!
//! Structures are transcribed from the official `limine-protocol` PROTOCOL.md
//! and follow its C ABI exactly. Only the features the kernel actually uses are
//! declared; unsupported requests are not emitted.
//!
//! The bootloader scans the loaded image for request IDs, fills in `response`
//! pointers, and (for base revision >= 3) rewrites the base-revision tag to
//! report the revision it actually used.

use core::cell::UnsafeCell;
use core::ptr;

use os_core::LIMINE_BASE_REVISION;

pub const COMMON_MAGIC_0: u64 = 0xc7b1dd30df4c8b88;
pub const COMMON_MAGIC_1: u64 = 0x0a82e883a194f07b;
const BASE_REVISION_MAGIC_0: u64 = 0xf9562b2d5c95a6c8;
const BASE_REVISION_MAGIC_1: u64 = 0x6a7b384944536bdc;

#[repr(C)]
pub struct Request<R> {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut R,
}

impl<R> Request<R> {
    const fn new(id3: u64, id4: u64) -> Self {
        Self {
            id: [COMMON_MAGIC_0, COMMON_MAGIC_1, id3, id4],
            revision: 0,
            response: ptr::null_mut(),
        }
    }

    pub fn get(&self) -> Option<&'static R> {
        // SAFETY: when non-null, the bootloader filled this pointer with a
        // response that lives in bootloader-reclaimable memory for the whole
        // kernel lifetime (we never reclaim it in this milestone).
        unsafe { self.response.as_ref() }
    }
}

#[repr(C)]
pub struct BootloaderInfoResponse {
    pub revision: u64,
    pub name: *const u8,
    pub version: *const u8,
}

#[repr(C)]
pub struct FirmwareTypeResponse {
    pub revision: u64,
    pub firmware_type: u64,
}

pub const FIRMWARE_X86_BIOS: u64 = 0;
pub const FIRMWARE_EFI32: u64 = 1;
pub const FIRMWARE_EFI64: u64 = 2;

#[repr(C)]
pub struct StackSizeRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut StackSizeResponse,
    pub stack_size: u64,
}

#[repr(C)]
pub struct StackSizeResponse {
    pub revision: u64,
}

#[repr(C)]
pub struct PagingModeRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut PagingModeResponse,
    pub mode: u64,
    pub max_mode: u64,
    pub min_mode: u64,
}

#[repr(C)]
pub struct PagingModeResponse {
    pub revision: u64,
    pub mode: u64,
}

impl PagingModeRequest {
    pub fn get(&self) -> Option<&'static PagingModeResponse> {
        // SAFETY: same contract as `Request::get`.
        unsafe { self.response.as_ref() }
    }
}

pub const PAGING_MODE_X86_64_4LVL: u64 = 0;
pub const PAGING_MODE_X86_64_5LVL: u64 = 1;

#[repr(C)]
pub struct HhdmResponse {
    pub revision: u64,
    pub offset: u64,
}

#[repr(C)]
pub struct RsdpResponse {
    pub revision: u64,
    pub address: *mut u8,
}

#[repr(C)]
pub struct CmdlineResponse {
    pub revision: u64,
    pub cmdline: *const u8,
}

#[repr(C)]
pub struct ExecutableAddressResponse {
    pub revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[repr(C)]
pub struct MemmapResponse {
    pub revision: u64,
    pub entry_count: u64,
    pub entries: *const *const MemmapEntry,
}

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

#[repr(C)]
pub struct FramebufferResponse {
    pub revision: u64,
    pub framebuffer_count: u64,
    pub framebuffers: *const *const Framebuffer,
}

#[repr(C)]
pub struct Framebuffer {
    pub address: *mut u8,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
    pub unused: [u8; 7],
    pub edid_size: u64,
    pub edid: *mut u8,
    pub mode_count: u64,
    pub modes: *const *const VideoMode,
}

#[repr(C)]
pub struct VideoMode {
    pub pitch: u64,
    pub width: u64,
    pub height: u64,
    pub bpp: u16,
    pub memory_model: u8,
    pub red_mask_size: u8,
    pub red_mask_shift: u8,
    pub green_mask_size: u8,
    pub green_mask_shift: u8,
    pub blue_mask_size: u8,
    pub blue_mask_shift: u8,
}

#[repr(C)]
pub struct TscFrequencyResponse {
    pub revision: u64,
    pub frequency: u64,
}

#[repr(C)]
pub struct Uuid {
    pub a: u32,
    pub b: u16,
    pub c: u16,
    pub d: [u8; 8],
}

/// `limine_file` (112 bytes): one module or the executable itself. `address`
/// is HHDM-virtual for modules; `path` is a NUL-terminated ASCII path with a
/// leading slash.
#[repr(C)]
pub struct File {
    pub revision: u64,
    pub address: *mut u8,
    pub size: u64,
    pub path: *const u8,
    pub string: *const u8,
    pub media_type: u32,
    pub unused: u32,
    pub tftp_ipv4: [u8; 4],
    pub tftp_port: u32,
    pub partition_index: u32,
    pub mbr_disk_id: u32,
    pub gpt_disk_uuid: Uuid,
    pub gpt_part_uuid: Uuid,
    pub part_uuid: Uuid,
}

#[repr(C)]
pub struct ModuleResponse {
    pub revision: u64,
    pub module_count: u64,
    pub modules: *const *const File,
}

#[repr(C)]
pub struct ModuleRequest {
    pub id: [u64; 4],
    pub revision: u64,
    pub response: *mut ModuleResponse,
    // Request revision 1 fields; declared for ABI completeness while the
    // kernel requests revision 0, so the bootloader leaves them as-is.
    pub internal_module_count: u64,
    pub internal_modules: *mut *const File,
}

impl ModuleRequest {
    pub fn get(&self) -> Option<&'static ModuleResponse> {
        // SAFETY: same contract as `Request::get`.
        unsafe { self.response.as_ref() }
    }
}

/// All requests emitted by the kernel. Field order defines the section layout;
/// requests may appear in any order in the image.
#[repr(C)]
pub struct Requests {
    base_revision: [u64; 3],
    pub bootloader_info: Request<BootloaderInfoResponse>,
    pub firmware_type: Request<FirmwareTypeResponse>,
    pub stack_size: StackSizeRequest,
    pub paging_mode: PagingModeRequest,
    pub hhdm: Request<HhdmResponse>,
    pub rsdp: Request<RsdpResponse>,
    pub executable_cmdline: Request<CmdlineResponse>,
    pub executable_address: Request<ExecutableAddressResponse>,
    pub memmap: Request<MemmapResponse>,
    pub framebuffer: Request<FramebufferResponse>,
    pub tsc_frequency: Request<TscFrequencyResponse>,
    pub modules: ModuleRequest,
}

impl Requests {
    const fn new() -> Self {
        Self {
            // The bootloader rewrites [1] with the revision it used and [2]
            // with zero when it supports that revision.
            base_revision: [
                BASE_REVISION_MAGIC_0,
                BASE_REVISION_MAGIC_1,
                LIMINE_BASE_REVISION,
            ],
            bootloader_info: Request::new(0xf55038d8e2a1202f, 0x279426fcf5f59740),
            firmware_type: Request::new(0x8c2f75d90bef28a8, 0x7045a4688eac00c3),
            stack_size: StackSizeRequest {
                id: [
                    COMMON_MAGIC_0,
                    COMMON_MAGIC_1,
                    0x224ef0460a8e8926,
                    0xe1cb0fc25f46ea3d,
                ],
                revision: 0,
                response: ptr::null_mut(),
                stack_size: 256 * 1024,
            },
            paging_mode: PagingModeRequest {
                id: [
                    COMMON_MAGIC_0,
                    COMMON_MAGIC_1,
                    0x95c1a0edab0944cb,
                    0xa4e5cb3842f7488a,
                ],
                revision: 1,
                response: ptr::null_mut(),
                mode: PAGING_MODE_X86_64_4LVL,
                max_mode: PAGING_MODE_X86_64_4LVL,
                min_mode: PAGING_MODE_X86_64_4LVL,
            },
            hhdm: Request::new(0x48dcf1cb8ad2b852, 0x63984e959a98244b),
            rsdp: Request::new(0xc5e77b6b397e7b43, 0x27637845accdcf3c),
            executable_cmdline: Request::new(0x4b161536e598651e, 0xb390ad4a2f1f303a),
            executable_address: Request::new(0x71ba76863cc55f63, 0xb2644a48c516a487),
            memmap: Request::new(0x67cf3d9d378a806f, 0xe304acdfc50c3c62),
            framebuffer: Request::new(0x9d5827dcd881dd75, 0xa3148604f6fab11b),
            tsc_frequency: Request::new(0x10f2ee1d87d195e4, 0xf747a2b78f6ddb31),
            modules: ModuleRequest {
                id: [
                    COMMON_MAGIC_0,
                    COMMON_MAGIC_1,
                    0x3e7e279702be32af,
                    0xca1c4f3bd1280cee,
                ],
                revision: 0,
                response: ptr::null_mut(),
                internal_module_count: 0,
                internal_modules: ptr::null_mut(),
            },
        }
    }
}

/// Interior mutability wrapper so the bootloader-patched request block can be a
/// single static. The kernel is single-core in this milestone and reads the
/// block only after the bootloader has finished writing it.
struct RequestCell(UnsafeCell<Requests>);

// SAFETY: the bootloader writes the block before the kernel starts; afterwards
// the kernel only reads it. No Rust code ever mutates it concurrently.
unsafe impl Sync for RequestCell {}

#[used]
#[unsafe(link_section = ".limine_requests")]
static REQUESTS: RequestCell = RequestCell(UnsafeCell::new(Requests::new()));

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static REQUESTS_START: [u64; 4] = [
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
];

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

pub fn get() -> &'static Requests {
    // SAFETY: see RequestCell.
    unsafe { &*REQUESTS.0.get() }
}

/// True when the bootloader understood and honoured the requested base
/// revision. A bootloader that does not support it leaves the third tag
/// component untouched (our requested revision), per the protocol.
pub fn base_revision_supported() -> bool {
    // Volatile read: the value is patched by the bootloader after link time.
    unsafe { ptr::read_volatile(ptr::addr_of!(get().base_revision[2])) == 0 }
}

/// The revision the bootloader reports using, when available.
pub fn loaded_base_revision() -> Option<u64> {
    let reported = unsafe { ptr::read_volatile(ptr::addr_of!(get().base_revision[1])) };
    if reported == BASE_REVISION_MAGIC_1 {
        None
    } else {
        Some(reported)
    }
}

/// Read a NUL-terminated bootloader string with a hard length cap. Bootloader
/// strings are trusted to be valid but we never scan unbounded memory.
pub fn cstr(pointer: *const u8) -> &'static str {
    if pointer.is_null() {
        return "<null>";
    }
    const MAX: usize = 1024;
    let mut length = 0usize;
    // SAFETY: the protocol guarantees a NUL-terminated string at this address.
    while length < MAX && unsafe { ptr::read(pointer.add(length)) } != 0 {
        length += 1;
    }
    // SAFETY: bounded above; protocol strings are ASCII.
    let bytes = unsafe { core::slice::from_raw_parts(pointer, length) };
    core::str::from_utf8(bytes).unwrap_or("<invalid-utf8>")
}

/// The module response as a slice of pointers, or `None` when the bootloader
/// provided no response. Per the protocol a response is only provided when at
/// least one module is present, and "no modules available" means no response.
pub fn modules() -> Option<&'static [*const File]> {
    let response = get().modules.get()?;
    if response.modules.is_null() || response.module_count == 0 {
        return Some(&[]);
    }
    let count = response.module_count as usize;
    // SAFETY: the protocol guarantees `modules` points to `module_count`
    // valid pointers while the response lives.
    Some(unsafe { core::slice::from_raw_parts(response.modules, count) })
}

/// Exact-match lookup by the module path given in `limine.conf`
/// (`/boot/hello.elf` etc.). Exact matching keeps a mis-staged ISO visible.
pub fn module_by_path(path: &str) -> Option<&'static File> {
    for &file in modules()? {
        if file.is_null() {
            continue;
        }
        // SAFETY: non-null array entries point to valid `limine_file` structs.
        let file = unsafe { &*file };
        if cstr(file.path) == path {
            return Some(file);
        }
    }
    None
}
