//! Classic ATA PIO block device driver (Phase 6, stage 2).
//!
//! Primary IDE channel, master drive, LBA28, polled PIO. The register
//! protocol and the machine-type rationale (PIIX southbridge ports
//! 0x1F0/0x3F6; QEMU `q35` exposes only ICH9-AHCI, which needs PCI) are
//! documented in `docs/PERSISTENCE.md` and `docs/RESEARCH.md` (R-009).
//!
//! There is no IRQ14 handler and no DMA: every wait polls the alternate
//! status register with a real-time deadline taken from the PIT, so the boot
//! path that calls [`init`] must run after the timer is ticking.

use alloc::string::String;

use crate::arch::x86_64::io::{inb, inw, outb, outw};
use crate::arch::x86_64::pit;
use crate::sync::IrqSpinLock;

const DATA: u16 = 0x1F0;
const SECTOR_COUNT: u16 = 0x1F2;
const LBA_LOW: u16 = 0x1F3;
const LBA_MID: u16 = 0x1F4;
const LBA_HIGH: u16 = 0x1F5;
const DRIVE: u16 = 0x1F6;
const STATUS: u16 = 0x1F7;
const COMMAND: u16 = 0x1F7;
const ALT_STATUS: u16 = 0x3F6;

const CMD_IDENTIFY: u8 = 0xEC;
const CMD_READ: u8 = 0x20;
const CMD_WRITE: u8 = 0x30;
const CMD_FLUSH: u8 = 0xE7;

const STATUS_ERR: u8 = 0x01;
const STATUS_DRQ: u8 = 0x08;
const STATUS_DF: u8 = 0x20;
const STATUS_BSY: u8 = 0x80;

/// Drive/head register (0x1F6) for the master in 28-bit LBA mode:
/// bit 7 = 1, bit 6 = LBA, bit 5 = 1, bit 4 = master; bits 3..=0 carry
/// LBA bits 24..=27. 0xA0 would select the master in CHS mode, which the
/// drive then translates with its geometry (a real bug this driver had).
const DRIVE_MASTER_LBA: u8 = 0xE0;
const SECTOR_BYTES: usize = 512;
const MAX_CHUNK_SECTORS: u32 = 255;
const LBA28_LIMIT: u32 = 0x0FFF_FFFF;

/// Wait budget for one poll loop: 5 s at the 100 Hz PIT.
const TIMEOUT_TICKS: u64 = 500;

#[derive(Clone, Debug)]
pub struct BlkInfo {
    pub model: String,
    pub sectors: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BlkStats {
    pub sectors_read: u32,
    pub sectors_written: u32,
    pub flushes: u32,
    pub errors: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlkError {
    NoDevice,
    Timeout,
    DeviceError(u8),
    BadRange,
}

impl BlkError {
    pub fn label(&self) -> &'static str {
        match self {
            BlkError::NoDevice => "no device",
            BlkError::Timeout => "timeout",
            BlkError::DeviceError(_) => "device error",
            BlkError::BadRange => "bad range",
        }
    }
}

#[derive(Default)]
struct State {
    info: Option<BlkInfo>,
    stats: BlkStats,
}

static STATE: IrqSpinLock<State> = IrqSpinLock::new(State {
    info: None,
    stats: BlkStats {
        sectors_read: 0,
        sectors_written: 0,
        flushes: 0,
        errors: 0,
    },
});

enum Probe {
    None,
    Atapi,
    Device(BlkInfo),
}

fn error_count() {
    let mut state = STATE.lock();
    state.stats.errors = state.stats.errors.saturating_add(1);
}

/// Four reads of the alternate status register: the documented ~400 ns
/// settle delay after a drive-select or command write.
fn delay_400ns() {
    for _ in 0..4 {
        let _ = inb(ALT_STATUS);
    }
}

fn deadline() -> u64 {
    pit::ticks() + TIMEOUT_TICKS
}

/// Poll until BSY clears; optionally require DRQ. ERR/DF fail a request.
fn wait_ready(need_drq: bool) -> Result<(), BlkError> {
    let limit = deadline();
    loop {
        let status = inb(ALT_STATUS);
        if status & STATUS_BSY == 0 {
            if status & (STATUS_ERR | STATUS_DF) != 0 {
                error_count();
                return Err(BlkError::DeviceError(status));
            }
            if !need_drq || status & STATUS_DRQ != 0 {
                return Ok(());
            }
        }
        if pit::ticks() > limit {
            error_count();
            return Err(BlkError::Timeout);
        }
        core::hint::spin_loop();
    }
}

fn select_drive(lba: u32) {
    outb(DRIVE, DRIVE_MASTER_LBA | ((lba >> 24) as u8 & 0x0F));
    delay_400ns();
}

fn identify() -> Probe {
    // Plain master select (CHS bit clear is irrelevant for IDENTIFY, which
    // does not use the LBA registers); the spec pins 0xA0 here.
    outb(DRIVE, 0xA0);
    outb(SECTOR_COUNT, 0);
    outb(LBA_LOW, 0);
    outb(LBA_MID, 0);
    outb(LBA_HIGH, 0);
    outb(COMMAND, CMD_IDENTIFY);
    delay_400ns();

    // A floating bus reads as zero; real devices set BSY or DRQ.
    if inb(STATUS) == 0 {
        return Probe::None;
    }
    if wait_ready(false).is_err() {
        // The device errored: distinguish ATAPI (sector-count/mid signature)
        // from a dead port.
        let mid = inb(LBA_MID);
        let high = inb(LBA_HIGH);
        if mid == 0x14 && high == 0xEB {
            return Probe::Atapi;
        }
        return Probe::None;
    }
    if inb(ALT_STATUS) & STATUS_DRQ == 0 {
        return Probe::None;
    }

    let mut words = [0u16; 256];
    for word in &mut words {
        *word = inw(DATA);
    }

    let sectors = (words[60] as u32) | ((words[61] as u32) << 16);
    if sectors == 0 {
        return Probe::None;
    }

    // Words 27..=46 hold the model as byte-swapped ASCII pairs.
    let mut model = String::new();
    for &word in &words[27..47] {
        model.push((word >> 8) as u8 as char);
        model.push((word & 0xFF) as u8 as char);
    }
    let model = String::from(model.trim_end_matches([' ', '\0']));

    Probe::Device(BlkInfo { model, sectors })
}

/// Probe the primary master once at boot. Logs exactly one pinned line.
pub fn init() -> Option<BlkInfo> {
    let probe = identify();
    let mut state = STATE.lock();
    match probe {
        Probe::None => {
            crate::kprintln!("blk: no ATA device on primary channel");
            state.info = None;
            None
        }
        Probe::Atapi => {
            crate::kprintln!("blk: ATAPI device on primary master (unsupported)");
            state.info = None;
            None
        }
        Probe::Device(info) => {
            crate::kprintln!("blk: found {}, {} sectors", info.model, info.sectors);
            state.info = Some(info.clone());
            Some(info)
        }
    }
}

pub fn info() -> Option<BlkInfo> {
    STATE.lock().info.clone()
}

pub fn stats() -> BlkStats {
    STATE.lock().stats
}

fn range_is_valid(lba: u32, sectors: u32) -> bool {
    sectors > 0
        && lba
            .checked_add(sectors)
            .is_some_and(|end| end <= LBA28_LIMIT)
}

fn transfer_begin(lba: u32, sectors: u32, command: u8) {
    select_drive(lba);
    outb(SECTOR_COUNT, sectors as u8);
    outb(LBA_LOW, lba as u8);
    outb(LBA_MID, (lba >> 8) as u8);
    outb(LBA_HIGH, (lba >> 16) as u8);
    outb(COMMAND, command);
    delay_400ns();
}

/// Read `buf.len() / 512` sectors starting at `lba`.
pub fn read_sectors(lba: u32, buf: &mut [u8]) -> Result<(), BlkError> {
    if buf.is_empty() || !buf.len().is_multiple_of(SECTOR_BYTES) {
        return Err(BlkError::BadRange);
    }
    if STATE.lock().info.is_none() {
        return Err(BlkError::NoDevice);
    }

    let mut sectors = (buf.len() / SECTOR_BYTES) as u32;
    if !range_is_valid(lba, sectors) {
        return Err(BlkError::BadRange);
    }

    let mut offset = 0usize;
    let mut next_lba = lba;
    let mut transferred = 0u32;
    while sectors > 0 {
        let chunk = sectors.min(MAX_CHUNK_SECTORS);
        transfer_begin(next_lba, chunk, CMD_READ);
        for _ in 0..chunk {
            wait_ready(true)?;
            for _ in 0..SECTOR_BYTES / 2 {
                let word = inw(DATA);
                buf[offset] = word as u8;
                buf[offset + 1] = (word >> 8) as u8;
                offset += 2;
            }
            transferred += 1;
        }
        next_lba += chunk;
        sectors -= chunk;
    }
    let mut state = STATE.lock();
    state.stats.sectors_read = state.stats.sectors_read.saturating_add(transferred);
    Ok(())
}

/// Write `buf.len() / 512` sectors starting at `lba`.
pub fn write_sectors(lba: u32, buf: &[u8]) -> Result<(), BlkError> {
    if buf.is_empty() || !buf.len().is_multiple_of(SECTOR_BYTES) {
        return Err(BlkError::BadRange);
    }
    if STATE.lock().info.is_none() {
        return Err(BlkError::NoDevice);
    }

    let mut sectors = (buf.len() / SECTOR_BYTES) as u32;
    if !range_is_valid(lba, sectors) {
        return Err(BlkError::BadRange);
    }

    let mut offset = 0usize;
    let mut next_lba = lba;
    let mut transferred = 0u32;
    while sectors > 0 {
        let chunk = sectors.min(MAX_CHUNK_SECTORS);
        transfer_begin(next_lba, chunk, CMD_WRITE);
        for _ in 0..chunk {
            wait_ready(true)?;
            for _ in 0..SECTOR_BYTES / 2 {
                let word = (buf[offset] as u16) | ((buf[offset + 1] as u16) << 8);
                outw(DATA, word);
                offset += 2;
            }
            transferred += 1;
        }
        next_lba += chunk;
        sectors -= chunk;
    }

    // Flush the device write cache so a crash cannot reorder writes.
    flush()?;

    let mut state = STATE.lock();
    state.stats.sectors_written = state.stats.sectors_written.saturating_add(transferred);
    Ok(())
}

/// Cache flush (0xE7): the ordering barrier after every slot write.
pub fn flush() -> Result<(), BlkError> {
    if STATE.lock().info.is_none() {
        return Err(BlkError::NoDevice);
    }
    outb(COMMAND, CMD_FLUSH);
    delay_400ns();
    wait_ready(false)?;
    let mut state = STATE.lock();
    state.stats.flushes = state.stats.flushes.saturating_add(1);
    Ok(())
}
