# Graphics, drivers and NVIDIA research

Status: **boot framebuffer only.** There is no GPU driver, no 2D/3D acceleration,
no native graphics API and no Windows graphics compatibility. The dashboard is
software drawing into the framebuffer Limine provides.

## Capability matrix

| Capability | Label | Evidence |
|---|---|---|
| Boot framebuffer | **framebuffer-only** | 1280x800x32 linear framebuffer drawn and captured in QEMU |
| Virtual graphics adapter (QEMU/VirtualBox) | **framebuffer-only** | QEMU VGA framebuffer used; no adapter driver |
| Basic native display driver | not started | no device enumeration, mode setting or connectors |
| 2D acceleration | not started | no command submission or blits |
| 3D acceleration | not started | no GPU memory manager, queues, shaders or fences |
| Native graphics API (OpenGL/Vulkan/etc.) | not started | none |
| Windows user-mode graphics (GDI/DXGI/D3D) | not started | none |
| Windows kernel-mode display driver (WDDM) | not started / research | none |

A device is not "supported" because PCI enumeration finds it—enumeration itself
is not implemented yet. The labels above are deliberate.

## What the boot framebuffer does

`kernel/src/display.rs`:

- validates width, height, pitch/stride, bpp, channel masks and size overflow via
  `os_core::fb`; refuses to draw on invalid geometry;
- supports 16/24/32 bpp packed formats with channel masks;
- draws rectangles and text using an original 5x7 bitmap font at integer scales;
- renders the project name, build id, eight status rows and a success/failure
  banner;
- is strictly secondary to serial: no framebuffer means a warning, not a failed
  boot.

Host tests cover geometry validation, pitch, bounds, overflow, colour packing,
console grid math and short-pitch rejection. The QEMU framebuffer screenshot was
captured via the monitor `screendump` command and its palette/layout inspected;
see `docs/assets/boot-screen.png`.

This is **not** a display driver: it uses the bootloader's linear framebuffer and
does no mode setting, vblank, page flip or hardware ownership.

## Planned graphics architecture (design only)

```text
Applications (native and, later, Windows)
        |
        v
graphics user-space runtime: surfaces, windows, presentation, API implementations
        |
        v
kernel graphics subsystem:
  device discovery/binding | GPU address space and memory | command submission
  fences/timeouts/reset     | display modes/connectors/scanout | IRQ and power
  process ownership and isolation
        |
        v
hardware drivers: VM adapter/software framebuffer, native drivers, audited ports,
                  future Windows WDDM-compatible environment
```

Interfaces will be introduced one at a time with a first real user and an ADR,
not as empty abstractions. Large API implementations belong in user space where
possible.

## NVIDIA routes (research)

### Route A — adapt open-source NVIDIA graphics (Nouveau / Mesa NVK)

- NVK is a Mesa Vulkan driver that documents Linux kernel/DRM requirements; it is
  not a standalone driver for an independent kernel.
- Nouveau's kernel-side functionality, firmware needs, supported generations and
  user-space dependencies must be mapped before any port.
- Requires a dependency graph, license audit (GPL obligations for kernel code)
  and a small isolated prototype before commitment.
- Sources: <https://nouveau.freedesktop.org/>, <https://docs.mesa3d.org/drivers/nvk.html>,
  <https://docs.mesa3d.org/>

### Route B — NVIDIA open kernel modules

- Are Linux kernel modules with Linux-specific dependencies, not portable code.
- Current README states support for Turing and later GPUs and GSP firmware /
  matching user-space requirements; verify the exact revision before relying on it.
- A source/dependency audit and a narrow build experiment are required. Do not
  download or redistribute blobs without checking license/distribution terms.
- Sources: <https://github.com/NVIDIA/open-gpu-kernel-modules>,
  <https://github.com/NVIDIA/open-gpu-kernel-modules/blob/main/README.md>

### Route C — Windows NVIDIA display-driver binaries

- Long-term compatibility objective, not the first implementation.
- Requires an original NT/WDM/WDDM-compatible environment: DDI contracts,
  memory management, scheduling, synchronization, interrupts, power, PnP and
  device lifecycle.
- Implement only for a deliberately scoped driver model/version, with isolation
  and recovery, and only after a test matrix exists.
- A `.sys` being x86-64 does not make it loadable.
- Sources: <https://learn.microsoft.com/en-us/windows-hardware/drivers/display/windows-vista-and-later-display-driver-model-architecture>,
  <https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/>

**Current recommendation (research-stage):** finish framebuffer/VM display and
define our own narrow graphics interfaces first; then evaluate Route A or B with
one named GPU target. Route C remains a separate high-risk research track. This
is a recommendation, not a porting commitment.

## Security and acceptance requirements

Before calling any physical GPU family "supported", require:

- named PCI IDs/supported range, firmware requirements, exact driver source
  revision and license review;
- repeatable init/teardown and reset tests;
- bounded behavior for invalid command buffers, timeouts and exhaustion;
- GPU virtual-address bounds and cross-process isolation;
- correct display modes/surfaces with no memory corruption;
- crash/hang recovery behavior and documented limitations;
- clear separation of VM-only success from physical-hardware success;
- graphics output correctness tests, not just "a desktop appeared".

Never flash firmware, modify host drivers or install GPU drivers as part of this
project's build workflow.

## Test entry points

| Script | Status |
|---|---|
| `scripts/test_graphics_host.sh` | PASS: framebuffer math tests |
| `scripts/test_boot_qemu.sh` | PASS: dashboard renders, serial marker independent |
| `scripts/verify_iso.py` | verifies ISO boot entries, not graphics |
| `scripts/test_pci.sh` | SKIP: PCI not implemented |
| `scripts/test_driver_model.sh` | SKIP: no driver framework |
| `scripts/test_gpu_vm.sh` | SKIP: framebuffer-only |
| `scripts/bench_graphics.sh` | SKIP: no accelerated implementation |

Do not report framebuffer drawing as GPU acceleration, and do not report a VM
framebuffer as a virtual GPU driver.
