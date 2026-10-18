//! Durable checkpoint image on the ATA disk (Phase 6, stage 2).
//!
//! The whole in-memory VFS tree is serialised with `os_core::pofs` into one of
//! two fixed 1 MiB slots (double buffering: a save always targets the slot
//! that is not active, so an interrupted write can only damage the inactive
//! copy). Boot loads the valid slot with the highest sequence number, or
//! formats slot A on a fresh disk. Shell mutations are flushed write-through
//! at command boundaries. The full contract is `docs/PERSISTENCE.md`.

use alloc::vec::Vec;

use os_core::pofs::{self, NodeRecord};

use crate::sync::IrqSpinLock;
use crate::{blk, kprintln, vfs};

/// Sectors per slot (1 MiB).
pub const SLOT_SECTORS: u32 = 2048;
/// First sector of slot A.
pub const SLOT_A_LBA: u32 = 16;
/// First sector of slot B.
pub const SLOT_B_LBA: u32 = SLOT_A_LBA + SLOT_SECTORS;
/// Smallest disk that can hold both slots.
pub const MIN_DISK_SECTORS: u32 = SLOT_B_LBA + SLOT_SECTORS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotId {
    A,
    B,
}

impl SlotId {
    pub fn name(self) -> &'static str {
        match self {
            SlotId::A => "A",
            SlotId::B => "B",
        }
    }

    fn lba(self) -> u32 {
        match self {
            SlotId::A => SLOT_A_LBA,
            SlotId::B => SLOT_B_LBA,
        }
    }

    fn other(self) -> SlotId {
        match self {
            SlotId::A => SlotId::B,
            SlotId::B => SlotId::A,
        }
    }
}

pub struct PersistStatus {
    pub active: Option<(SlotId, u64)>,
    pub saves: u32,
    pub loads: u32,
    pub errors: u32,
    pub last_load: Option<(SlotId, u64)>,
}

struct State {
    active: Option<SlotId>,
    seq: u64,
    saves: u32,
    loads: u32,
    errors: u32,
    last_load: Option<(SlotId, u64)>,
}

static STATE: IrqSpinLock<State> = IrqSpinLock::new(State {
    active: None,
    seq: 0,
    saves: 0,
    loads: 0,
    errors: 0,
    last_load: None,
});

struct Candidate {
    slot: SlotId,
    seq: u64,
    records: Vec<NodeRecord>,
    payload_len: u32,
}

/// Serialise the live tree; returns the payload and its node count.
fn encode_current_tree() -> Result<(Vec<u8>, usize), &'static str> {
    let records = vfs::with(|vfs| vfs.export()).map_err(|_| "vfs unavailable")?;
    let payload = pofs::encode_payload(&records).map_err(|error| error.label())?;
    Ok((payload, records.len()))
}

/// Header + zero-padded payload as one contiguous slot image.
fn build_slot_image(seq: u64, payload: &[u8]) -> Vec<u8> {
    let payload_crc = pofs::crc32(payload);
    let header = pofs::encode_header(seq, payload.len() as u32, payload_crc);
    let padded_len = payload.len().div_ceil(512) * 512;
    let mut image = Vec::with_capacity(pofs::HEADER_BYTES + padded_len);
    image.extend_from_slice(&header);
    image.extend_from_slice(payload);
    image.resize(pofs::HEADER_BYTES + padded_len, 0);
    image
}

/// Write a slot, then read the header back as a commit check.
fn store(slot: SlotId, seq: u64, payload: &[u8]) -> Result<(), &'static str> {
    let image = build_slot_image(seq, payload);
    blk::write_sectors(slot.lba(), &image).map_err(|error| error.label())?;
    let mut read_back = [0u8; 512];
    blk::read_sectors(slot.lba(), &mut read_back).map_err(|error| error.label())?;
    if read_back[..] != image[..512] {
        return Err("header read-back mismatch");
    }
    Ok(())
}

/// Read and validate one slot. `Ok(None)` means the slot is empty (all-zero
/// header); `Err(reason)` means it holds data that failed validation.
fn examine(slot: SlotId) -> Result<Option<Candidate>, &'static str> {
    let mut header_bytes = [0u8; 512];
    blk::read_sectors(slot.lba(), &mut header_bytes).map_err(|error| error.label())?;
    if pofs::is_empty_header(&header_bytes) {
        return Ok(None);
    }
    let header = pofs::parse_header(&header_bytes).map_err(|error| error.label())?;

    let payload_sectors = (header.payload_len as usize).div_ceil(512) as u32;
    let mut buffer = alloc::vec![0u8; payload_sectors as usize * 512];
    blk::read_sectors(slot.lba() + 1, &mut buffer).map_err(|error| error.label())?;
    let payload = &buffer[..header.payload_len as usize];
    if pofs::crc32(payload) != header.payload_crc {
        return Err("payload crc mismatch");
    }
    let records = pofs::decode_payload(payload).map_err(|error| error.label())?;
    Ok(Some(Candidate {
        slot,
        seq: header.seq,
        records,
        payload_len: header.payload_len,
    }))
}

fn set_active(slot: SlotId, seq: u64, loaded: bool) {
    let mut state = STATE.lock();
    state.active = Some(slot);
    state.seq = seq;
    if loaded {
        state.loads = state.loads.saturating_add(1);
        state.last_load = Some((slot, seq));
    }
}

/// Boot: load the best valid checkpoint or format slot A on a fresh disk.
pub fn init() {
    let Some(info) = blk::info() else {
        kprintln!("persist: inactive (no ATA disk)");
        return;
    };
    if info.sectors < MIN_DISK_SECTORS {
        kprintln!(
            "persist: disk too small ({} sectors, need {})",
            info.sectors,
            MIN_DISK_SECTORS
        );
        return;
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for slot in [SlotId::A, SlotId::B] {
        match examine(slot) {
            Ok(None) => {}
            Ok(Some(candidate)) => candidates.push(candidate),
            Err(reason) => kprintln!("persist: slot {} invalid ({})", slot.name(), reason),
        }
    }
    candidates.sort_by_key(|candidate| core::cmp::Reverse(candidate.seq));

    for candidate in &candidates {
        match vfs::with(|vfs| vfs.replace(&candidate.records)) {
            Ok(Ok(())) => {
                set_active(candidate.slot, candidate.seq, true);
                kprintln!(
                    "persist: loaded slot {} seq={} ({} nodes, {} bytes, crc ok)",
                    candidate.slot.name(),
                    candidate.seq,
                    candidate.records.len(),
                    candidate.payload_len
                );
                return;
            }
            Ok(Err(error)) => {
                kprintln!(
                    "persist: slot {} invalid ({})",
                    candidate.slot.name(),
                    error.label()
                );
            }
            Err(reason) => {
                kprintln!(
                    "persist: slot {} invalid ({})",
                    candidate.slot.name(),
                    reason
                );
            }
        }
    }

    let (payload, _) = match encode_current_tree() {
        Ok(encoded) => encoded,
        Err(reason) => {
            kprintln!("persist: format failed ({})", reason);
            return;
        }
    };
    match store(SlotId::A, 0, &payload) {
        Ok(()) => {
            set_active(SlotId::A, 0, false);
            vfs::clear_dirty();
            kprintln!("persist: no valid checkpoint, formatted slot A seq=0");
        }
        Err(reason) => kprintln!("persist: format failed ({})", reason),
    }
}

/// Write-through flush at a shell-command boundary, when the tree is dirty.
pub fn flush_if_dirty() {
    let (active, seq) = {
        let state = STATE.lock();
        match state.active {
            Some(slot) => (slot, state.seq),
            None => return,
        }
    };
    if !vfs::dirty() {
        return;
    }

    let (payload, nodes) = match encode_current_tree() {
        Ok(encoded) => encoded,
        Err(reason) => {
            let mut state = STATE.lock();
            state.errors = state.errors.saturating_add(1);
            drop(state);
            kprintln!(
                "persist: save failed ({}); previous checkpoint kept",
                reason
            );
            return;
        }
    };

    let target = active.other();
    let next_seq = seq + 1;
    match store(target, next_seq, &payload) {
        Ok(()) => {
            let mut state = STATE.lock();
            state.active = Some(target);
            state.seq = next_seq;
            state.saves = state.saves.saturating_add(1);
            drop(state);
            vfs::clear_dirty();
            kprintln!(
                "persist: saved slot {} seq={} ({} nodes, {} bytes)",
                target.name(),
                next_seq,
                nodes,
                payload.len()
            );
        }
        Err(reason) => {
            let mut state = STATE.lock();
            state.errors = state.errors.saturating_add(1);
            drop(state);
            kprintln!(
                "persist: save failed ({}); previous checkpoint kept",
                reason
            );
        }
    }
}

/// True when a usable checkpoint store is active (marker and shell use it).
pub fn active() -> bool {
    STATE.lock().active.is_some()
}

pub fn status() -> PersistStatus {
    let state = STATE.lock();
    PersistStatus {
        active: state.active.map(|slot| (slot, state.seq)),
        saves: state.saves,
        loads: state.loads,
        errors: state.errors,
        last_load: state.last_load,
    }
}
