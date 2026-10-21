//! Intel 82540EM ("e1000") NIC driver (Phase 6, stage 5).
//!
//! Polled, interrupt-free legacy-descriptor driver for the NIC QEMU's `pc`
//! machine and VirtualBox expose by default. Register semantics, the loopback
//! selftest rationale and the EEPROM fallback are documented in
//! `docs/NETWORK.md` §6 and `docs/RESEARCH.md` (R-012).
//!
//! All register and descriptor access is volatile. Descriptor rings and
//! buffers come from the frame allocator and are mapped through the HHDM;
//! device-visible addresses are the physical addresses of those frames.

use os_core::net::eth::{self, MacAddr};
use os_core::pci::BarKind;

use crate::arch::x86_64::pit;
use crate::memory;

const TWO_MIB: u64 = 2 * 1024 * 1024;
const MIN_BAR_BYTES: u64 = 128 * 1024;

// Register offsets (8254x SDM section 13.3).
const CTRL: u32 = 0x0000;
const EERD: u32 = 0x0014;
const ICR: u32 = 0x00C0;
const IMC: u32 = 0x00D8;
const RCTL: u32 = 0x0100;
const TCTL: u32 = 0x0400;
const TIPG: u32 = 0x0410;
const RDBAL: u32 = 0x2800;
const RDBAH: u32 = 0x2804;
const RDLEN: u32 = 0x2808;
const RDH: u32 = 0x2810;
const RDT: u32 = 0x2818;
const TDBAL: u32 = 0x3800;
const TDBAH: u32 = 0x3804;
const TDLEN: u32 = 0x3808;
const TDH: u32 = 0x3810;
const TDT: u32 = 0x3818;
const MTA: u32 = 0x5200;
const RAL: u32 = 0x5400;
const RAH: u32 = 0x5404;

const CTRL_FD: u32 = 1 << 0;
const CTRL_RST: u32 = 1 << 26;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_SLU: u32 = 1 << 6;

/// EN (bit 1). Broadcast accept (bit 15) is required for ARP requests;
/// SECRC (bit 26) strips the 4-byte FCS before the frame reaches memory.
const RCTL_VALUE: u32 = (1 << 1) | (1 << 15) | (1 << 26);
/// MAC-level loopback lives in RCTL.LBM (bits 7:6), 0x40 = LBM_MAC.
const RCTL_LBM_MAC: u32 = 0x40;

/// EN, pad short packets, retransmit on late collision, collision threshold
/// 0x10, collision distance 0x40, with a recommended default IPG.
const TCTL_VALUE: u32 = (1 << 1) | (1 << 3) | (0x10 << 4) | (0x40 << 12) | (1 << 24);
const TIPG_DEFAULT: u32 = 10 | (8 << 10) | (6 << 20);

const RAH_ADDRESS_VALID: u32 = 1 << 31;

/// Transmit command: end of packet, insert FCS, report status.
const TX_CMD_EOP_IFCS_RS: u8 = 0x01 | 0x02 | 0x08;
const DESC_STATUS_DD: u8 = 0x01;
const DESC_STATUS_EOP: u8 = 0x02;

const RX_DESCRIPTORS: usize = 32;
const RX_BUFFER_BYTES: usize = 2048;
const RX_PAGES: usize = RX_DESCRIPTORS * RX_BUFFER_BYTES / os_core::frame::PAGE_SIZE as usize;
/// Pages the driver allocates before the device is usable: TX descriptors,
/// TX buffer, RX descriptors and the RX buffer pages. Claimed as four exact
/// physical ranges in the driver resource ledger before the device is enabled.
pub const DMA_PAGE_TOTAL: u32 = 3 + RX_PAGES as u32;
const TX_DESCRIPTORS: usize = 8;
const TX_BUFFER_BYTES: usize = 2048;

/// Reset self-clears within ~1 s on real silicon; 5 s is generous.
const RESET_TIMEOUT_TICKS: u64 = 500;
/// One EEPROM word read; VirtualBox never raises DONE (see R-012).
const EEPROM_TIMEOUT_TICKS: u64 = 10;
const TX_TIMEOUT_TICKS: u64 = 10;
/// Secondary spin bound for transmit waits, which may run with interrupts
/// disabled (PIT frozen) inside the network lock.
const TX_SPIN_LIMIT: u32 = 20_000_000;
const LOOPBACK_TIMEOUT_TICKS: u64 = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Loopback {
    Passed,
    Unsupported,
    UnexpectedFrame,
}

#[repr(C, align(16))]
struct RxDescriptor {
    buffer_address: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C, align(16))]
struct TxDescriptor {
    buffer_address: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

/// All fields are plain integers (virtual/physical addresses as `usize`/`u64`)
/// so the driver is `Send` and can live inside the network lock's state.
pub struct E1000 {
    registers: usize,
    mac: MacAddr,
    tx_descriptors: usize,
    tx_descriptors_phys: u64,
    tx_buffer: usize,
    tx_buffer_phys: u64,
    tx_next: usize,
    rx_descriptors: usize,
    rx_descriptors_phys: u64,
    rx_buffers_phys: u64,
    rx_buffers: [(usize, u64); RX_DESCRIPTORS],
    rx_next: usize,
    tx_frames: u32,
    rx_frames: u32,
    errors: u32,
}

fn mac_is_valid(mac: &MacAddr) -> bool {
    let octets = mac.octets();
    octets.iter().any(|byte| *byte != 0) && octets.iter().any(|byte| *byte != 0xFF)
}

impl E1000 {
    fn read(&self, offset: u32) -> u32 {
        // SAFETY: offset is one of the register offsets above; the BAR0 window
        // is mapped uncached and at least 128 KiB long.
        unsafe { core::ptr::read_volatile((self.registers + offset as usize) as *const u32) }
    }

    fn write(&self, offset: u32, value: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.registers + offset as usize) as *mut u32, value) };
    }

    pub fn mac(&self) -> MacAddr {
        self.mac
    }

    /// `(tx_frames, rx_frames, errors)`.
    pub fn counters(&self) -> (u32, u32, u32) {
        (self.tx_frames, self.rx_frames, self.errors)
    }

    /// Bring up the NIC at `device`'s BAR0. `hhdm_offset` maps the physical
    /// window; must be called after the frame allocator and vm init.
    pub fn init<F>(
        device: &crate::pci::PciDevice,
        hhdm_offset: u64,
        claim: F,
    ) -> Result<E1000, &'static str>
    where
        F: FnOnce([(u64, u32); 4]) -> Result<(), &'static str>,
    {
        if !matches!(device.bar0.kind, BarKind::Memory32 | BarKind::Memory64) {
            return Err("bar0 is not a memory BAR");
        }
        if device.bar0_size < MIN_BAR_BYTES {
            return Err("bar0 is smaller than 128 KiB");
        }
        let aligned = device.bar0.base & !(TWO_MIB - 1);
        let virtual_base = hhdm_offset
            .checked_add(aligned)
            .ok_or("bar0 HHDM address overflowed")?;
        crate::arch::x86_64::paging::PageTables::new(hhdm_offset)
            .map_mmio_2m(aligned, virtual_base)
            .map_err(|_| "bar0 MMIO mapping failed")?;
        let registers = (hhdm_offset + device.bar0.base) as usize;

        let mut nic = E1000 {
            registers,
            mac: MacAddr::ZERO,
            tx_descriptors: 0,
            tx_descriptors_phys: 0,
            tx_buffer: 0,
            tx_buffer_phys: 0,
            tx_next: 0,
            rx_descriptors: 0,
            rx_descriptors_phys: 0,
            rx_buffers_phys: 0,
            rx_buffers: [(0, 0); RX_DESCRIPTORS],
            rx_next: 0,
            tx_frames: 0,
            rx_frames: 0,
            errors: 0,
        };
        nic.reset()?;
        nic.read_mac()?;
        nic.allocate_rings(hhdm_offset)?;
        nic.configure_rings();
        if let Err(reason) = claim(nic.dma_ranges()) {
            let _ = nic.free_rings();
            return Err(reason);
        }
        nic.configure_device();
        Ok(nic)
    }

    /// Physical DMA page ranges owned by this device: RX buffer run, TX
    /// descriptors, RX descriptors and the TX buffer.
    pub fn dma_ranges(&self) -> [(u64, u32); 4] {
        [
            (self.rx_buffers_phys, RX_PAGES as u32),
            (self.tx_descriptors_phys, 1),
            (self.rx_descriptors_phys, 1),
            (self.tx_buffer_phys, 1),
        ]
    }

    /// Stop the device and return every ring frame to the frame allocator.
    /// The HHDM MMIO mapping is kernel-lifetime and intentionally retained
    /// (there is no unmap API yet); a restart reuses it. If the reset before
    /// the free fails the rings are leaked rather than freed while the device
    /// may still be DMAing.
    pub fn shutdown(mut self) -> Result<(), &'static str> {
        self.write(IMC, u32::MAX);
        self.write(RCTL, 0);
        self.write(TCTL, 0);
        self.reset()?;
        self.free_rings()
    }

    /// Return the ring frames. Callers must have quiesced the device.
    fn free_rings(&self) -> Result<(), &'static str> {
        let mut error = None;
        for (physical, count) in [
            (self.rx_buffers_phys, RX_PAGES),
            (self.tx_descriptors_phys, 1),
            (self.rx_descriptors_phys, 1),
            (self.tx_buffer_phys, 1),
        ] {
            let Some(frame) = os_core::frame::PhysFrame::from_address(physical) else {
                error.get_or_insert("ring frame is not page aligned");
                continue;
            };
            if let Err(reason) = memory::free_pages(frame, count) {
                error.get_or_insert(reason);
            }
        }
        match error {
            None => Ok(()),
            Some(reason) => Err(reason),
        }
    }

    fn reset(&mut self) -> Result<(), &'static str> {
        self.write(CTRL, self.read(CTRL) | CTRL_RST);
        let limit = pit::ticks() + RESET_TIMEOUT_TICKS;
        while self.read(CTRL) & CTRL_RST != 0 {
            if pit::ticks() > limit {
                return Err("reset timeout");
            }
            core::hint::spin_loop();
        }
        Ok(())
    }

    /// Read the MAC from RAL/RAH (programmed by firmware/boot) or, when those
    /// are not valid, from the EEPROM words 0..=2. Programs RAL/RAH with the
    /// address-valid bit so unicast frames are accepted.
    fn read_mac(&mut self) -> Result<(), &'static str> {
        let mut mac = self.mac_from_ral_rah();
        if !mac_is_valid(&mac) {
            mac = self.mac_from_eeprom()?;
        }
        if !mac_is_valid(&mac) {
            return Err("no MAC address in RAL/RAH or EEPROM");
        }
        self.mac = mac;
        let octets = mac.octets();
        let ral = u32::from(octets[0])
            | (u32::from(octets[1]) << 8)
            | (u32::from(octets[2]) << 16)
            | (u32::from(octets[3]) << 24);
        let rah = u32::from(octets[4]) | (u32::from(octets[5]) << 8) | RAH_ADDRESS_VALID;
        self.write(RAL, ral);
        self.write(RAH, rah);
        Ok(())
    }

    fn mac_from_ral_rah(&self) -> MacAddr {
        let ral = self.read(RAL);
        let rah = self.read(RAH);
        MacAddr([
            ral as u8,
            (ral >> 8) as u8,
            (ral >> 16) as u8,
            (ral >> 24) as u8,
            rah as u8,
            (rah >> 8) as u8,
        ])
    }

    fn mac_from_eeprom(&self) -> Result<MacAddr, &'static str> {
        let word0 = self.eeprom_read(0)?;
        let word1 = self.eeprom_read(1)?;
        let word2 = self.eeprom_read(2)?;
        Ok(MacAddr([
            word0 as u8,
            (word0 >> 8) as u8,
            word1 as u8,
            (word1 >> 8) as u8,
            word2 as u8,
            (word2 >> 8) as u8,
        ]))
    }

    /// EERD.DONE is bit 4; not every emulated NIC raises it (R-012).
    fn eeprom_read(&self, address: u8) -> Result<u16, &'static str> {
        self.write(EERD, (u32::from(address) << 8) | 0x1);
        let limit = pit::ticks() + EEPROM_TIMEOUT_TICKS;
        loop {
            let value = self.read(EERD);
            if value & 0x10 != 0 {
                return Ok((value >> 16) as u16);
            }
            if pit::ticks() > limit {
                return Err("EEPROM read timeout");
            }
            core::hint::spin_loop();
        }
    }

    fn allocate_rings(&mut self, hhdm_offset: u64) -> Result<(), &'static str> {
        let tx_descriptors = memory::alloc_frame()?;
        self.tx_descriptors_phys = tx_descriptors.address();
        self.tx_descriptors = (hhdm_offset + self.tx_descriptors_phys) as usize;

        let tx_buffer = memory::alloc_frame()?;
        self.tx_buffer_phys = tx_buffer.address();
        self.tx_buffer = (hhdm_offset + self.tx_buffer_phys) as usize;

        let rx_descriptors = memory::alloc_frame()?;
        self.rx_descriptors_phys = rx_descriptors.address();
        self.rx_descriptors = (hhdm_offset + self.rx_descriptors_phys) as usize;

        let rx_buffers = memory::alloc_pages(RX_PAGES)?;
        self.rx_buffers_phys = rx_buffers.address();
        for (index, slot) in self.rx_buffers.iter_mut().enumerate() {
            let physical = rx_buffers.address() + (index * RX_BUFFER_BYTES) as u64;
            *slot = ((hhdm_offset + physical) as usize, physical);
        }

        // SAFETY: each frame is exclusively owned by this driver for the
        // lifetime of the kernel; zeroing descriptor memory before use.
        unsafe {
            core::ptr::write_bytes(
                self.tx_descriptors as *mut u8,
                0,
                os_core::frame::PAGE_SIZE as usize,
            );
            core::ptr::write_bytes(
                self.rx_descriptors as *mut u8,
                0,
                os_core::frame::PAGE_SIZE as usize,
            );
        }
        for index in 0..RX_DESCRIPTORS {
            let descriptor = self.rx_descriptor(index);
            let (_, physical) = self.rx_buffers[index];
            // SAFETY: descriptor is valid and 16-byte aligned; the device
            // owns the buffer once RCTL.EN is set.
            unsafe {
                core::ptr::write_volatile(
                    core::ptr::addr_of_mut!((*descriptor).buffer_address),
                    physical,
                );
                core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).status), 0);
            }
        }
        for index in 0..TX_DESCRIPTORS {
            let descriptor = self.tx_descriptor(index);
            // SAFETY: as above; a shared transmit buffer is fine because
            // sends are synchronous and complete before the next one starts.
            unsafe {
                core::ptr::write_volatile(
                    core::ptr::addr_of_mut!((*descriptor).buffer_address),
                    self.tx_buffer_phys,
                );
                core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).status), 0);
            }
        }
        Ok(())
    }

    fn configure_rings(&self) {
        self.write(RDBAL, self.rx_descriptors_phys as u32);
        self.write(RDBAH, (self.rx_descriptors_phys >> 32) as u32);
        self.write(
            RDLEN,
            (RX_DESCRIPTORS * core::mem::size_of::<RxDescriptor>()) as u32,
        );
        self.write(RDH, 0);
        self.write(RDT, (RX_DESCRIPTORS - 1) as u32);

        self.write(TDBAL, self.tx_descriptors_phys as u32);
        self.write(TDBAH, (self.tx_descriptors_phys >> 32) as u32);
        self.write(
            TDLEN,
            (TX_DESCRIPTORS * core::mem::size_of::<TxDescriptor>()) as u32,
        );
        self.write(TDH, 0);
        self.write(TDT, 0);
    }

    fn configure_device(&self) {
        self.write(IMC, u32::MAX);
        let _ = self.read(ICR);
        for index in 0..128u32 {
            self.write(MTA + index * 4, 0);
        }
        self.write(TIPG, TIPG_DEFAULT);
        self.write(TCTL, TCTL_VALUE);
        self.write(CTRL, CTRL_FD | CTRL_ASDE | CTRL_SLU);
        self.write(RCTL, RCTL_VALUE);
    }

    fn rx_descriptor(&self, index: usize) -> *mut RxDescriptor {
        (self.rx_descriptors as *mut RxDescriptor).wrapping_add(index)
    }

    fn tx_descriptor(&self, index: usize) -> *mut TxDescriptor {
        (self.tx_descriptors as *mut TxDescriptor).wrapping_add(index)
    }

    /// Transmit one frame synchronously through the shared buffer. Returns
    /// after the descriptor reports done, so the caller's slice is free when
    /// this returns.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.is_empty() || frame.len() > TX_BUFFER_BYTES {
            self.errors = self.errors.saturating_add(1);
            return Err("frame length out of range");
        }
        // SAFETY: the shared transmit buffer is owned by the driver and the
        // previous transmission completed (DD polled below) before reuse.
        unsafe {
            for (index, byte) in frame.iter().enumerate() {
                core::ptr::write_volatile((self.tx_buffer + index) as *mut u8, *byte);
            }
        }

        let slot = self.tx_next;
        let descriptor = self.tx_descriptor(slot);
        // SAFETY: descriptor slot is exclusively owned by the driver between
        // DD and the next enqueue; fields are written before TDT advances.
        unsafe {
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*descriptor).length),
                frame.len() as u16,
            );
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).cso), 0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).css), 0);
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).special), 0);
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*descriptor).cmd),
                TX_CMD_EOP_IFCS_RS,
            );
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).status), 0);
        }
        self.write(TDT, ((slot + 1) % TX_DESCRIPTORS) as u32);

        let limit = pit::ticks() + TX_TIMEOUT_TICKS;
        let mut spins: u32 = 0;
        loop {
            // SAFETY: the descriptor remains owned by the driver until done.
            let status =
                unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*descriptor).status)) };
            if status & DESC_STATUS_DD != 0 {
                // SAFETY: clearing DD hands the slot back to the driver.
                unsafe {
                    core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).status), 0);
                }
                self.tx_next = (slot + 1) % TX_DESCRIPTORS;
                self.tx_frames = self.tx_frames.saturating_add(1);
                return Ok(());
            }
            if pit::ticks() > limit || spins >= TX_SPIN_LIMIT {
                self.errors = self.errors.saturating_add(1);
                return Err("transmit timeout");
            }
            spins += 1;
            core::hint::spin_loop();
        }
    }

    /// Copy the next completed received frame into `buffer`. Returns the
    /// frame length, truncated to the buffer size. Frames with receive
    /// errors are released and skipped.
    pub fn receive_into(&mut self, buffer: &mut [u8]) -> Option<usize> {
        let index = self.rx_next;
        let descriptor = self.rx_descriptor(index);
        // SAFETY: descriptor is owned by the driver until re-armed below.
        let status = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*descriptor).status)) };
        if status & DESC_STATUS_DD == 0 {
            return None;
        }
        // SAFETY: as above.
        let errors = unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*descriptor).errors)) };
        // SAFETY: as above.
        let length =
            unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*descriptor).length)) } as usize;
        let complete = status & DESC_STATUS_EOP != 0;
        let (source, _) = self.rx_buffers[index];
        if errors != 0 || !complete {
            self.errors = self.errors.saturating_add(1);
            self.rearm_rx(index);
            return None;
        }
        let copy = length.min(buffer.len());
        for (offset, byte) in buffer[..copy].iter_mut().enumerate() {
            // SAFETY: the device wrote `length` bytes into this buffer and
            // the descriptor is still owned by the driver.
            *byte = unsafe { core::ptr::read_volatile((source + offset) as *const u8) };
        }
        self.rx_frames = self.rx_frames.saturating_add(1);
        self.rearm_rx(index);
        Some(copy)
    }

    fn rearm_rx(&mut self, index: usize) {
        let descriptor = self.rx_descriptor(index);
        // SAFETY: clearing the descriptor status and moving the tail pointer
        // hands the slot back to the device (follows the SDM's sample flow:
        // after consuming descriptor i, RDT = i).
        unsafe {
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*descriptor).status), 0);
        }
        self.write(RDT, (index % RX_DESCRIPTORS) as u32);
        self.rx_next = (index + 1) % RX_DESCRIPTORS;
    }

    /// MAC-level loopback selftest: transmit one 60-byte frame to ourselves
    /// with RCTL.LBM_MAC set and check that it comes back unchanged. Never
    /// fatal — emulated hardware may not implement loopback.
    pub fn loopback_selftest(&mut self) -> Loopback {
        const ETHERTYPE_TEST: u16 = 0x88B5;
        let mut frame = [0u8; 60];
        eth::Header::write(&mut frame, self.mac, self.mac, ETHERTYPE_TEST)
            .expect("60-byte buffer holds an Ethernet header");
        for (index, byte) in frame[eth::HEADER_LEN..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(31).wrapping_add(7);
        }

        self.write(RCTL, self.read(RCTL) | RCTL_LBM_MAC);
        let mut outcome = Loopback::Unsupported;
        if self.transmit(&frame).is_ok() {
            let limit = pit::ticks() + LOOPBACK_TIMEOUT_TICKS;
            let mut buffer = [0u8; 64];
            while pit::ticks() <= limit {
                match self.receive_into(&mut buffer) {
                    Some(length) if length == frame.len() && buffer[..frame.len()] == frame => {
                        outcome = Loopback::Passed;
                        break;
                    }
                    Some(_) => {
                        outcome = Loopback::UnexpectedFrame;
                        break;
                    }
                    None => core::hint::spin_loop(),
                }
            }
        }
        self.write(RCTL, self.read(RCTL) & !RCTL_LBM_MAC);
        outcome
    }
}
