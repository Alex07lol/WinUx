//! ACPI RSDP, root-table and MADT parsing.
//!
//! This module is deliberately allocation-free and pointer-free: callers give
//! it bytes from a table they have already validated and mapped. It checks
//! every checksum, declared length and MADT entry bound before exposing a
//! value, so a malformed firmware table becomes an error rather than a read
//! past the supplied slice.
//!
//! PCI `_PRT` is intentionally absent. QEMU q35 emits `_PRT` as an AML method,
//! which needs an AML evaluator; this stage only discovers the IOAPIC and ISA
//! interrupt-source overrides.

use crate::ioapic::{Polarity, TriggerMode};

pub const RSDP_MIN_LEN: usize = 20;
pub const RSDP_V2_LEN: usize = 36;
pub const SDT_HEADER_LEN: usize = 36;
pub const MADT_HEADER_LEN: usize = 44;
pub const MAX_IOAPICS: usize = 4;
pub const MAX_ISA_OVERRIDES: usize = 16;

const RSDP_SIGNATURE: &[u8; 8] = b"RSD PTR ";
const MADT_SIGNATURE: &[u8; 4] = b"APIC";
const XSDT_SIGNATURE: &[u8; 4] = b"XSDT";
const RSDT_SIGNATURE: &[u8; 4] = b"RSDT";

/// A rejected ACPI structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpiError {
    Truncated,
    BadSignature,
    BadChecksum,
    BadLength,
    BadAlignment,
    BadEntry,
    TooManyIoApics,
    TooManyOverrides,
    DuplicateIoApic,
    DuplicateOverride,
    ReservedFlags,
    NoRootTable,
}

impl AcpiError {
    /// Short reason suitable for a serial diagnostic.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Truncated => "truncated ACPI structure",
            Self::BadSignature => "bad ACPI signature",
            Self::BadChecksum => "bad ACPI checksum",
            Self::BadLength => "invalid ACPI length",
            Self::BadAlignment => "IOAPIC address is not page aligned",
            Self::BadEntry => "invalid MADT entry",
            Self::TooManyIoApics => "too many IOAPICs",
            Self::TooManyOverrides => "too many ISA overrides",
            Self::DuplicateIoApic => "duplicate IOAPIC address",
            Self::DuplicateOverride => "duplicate ISA override",
            Self::ReservedFlags => "reserved MADT interrupt flags",
            Self::NoRootTable => "no ACPI root table",
        }
    }
}

/// The subset of the RSDP this kernel consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsdp {
    pub revision: u8,
    pub rsdt_address: u32,
    pub xsdt_address: Option<u64>,
}

/// A validated SDT header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
}

/// A validated XSDT/RSDT entry list.
#[derive(Clone, Copy, Debug)]
pub struct TableList<'a> {
    bytes: &'a [u8],
    entry_size: usize,
    count: usize,
}

impl TableList<'_> {
    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Physical address of entry `index`, or `None` when out of range.
    pub fn address(&self, index: usize) -> Option<u64> {
        if index >= self.count {
            return None;
        }
        let offset = SDT_HEADER_LEN + index * self.entry_size;
        if self.entry_size == 8 {
            read_u64(self.bytes, offset)
        } else {
            read_u32(self.bytes, offset).map(u64::from)
        }
    }
}

/// One MADT IOAPIC entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoApicEntry {
    pub id: u8,
    pub address: u32,
    pub gsi_base: u32,
}

/// One MADT bus-0 ISA interrupt-source override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsaOverride {
    pub bus: u8,
    pub source: u8,
    pub gsi: u32,
    pub flags: u16,
}

impl IsaOverride {
    /// True when both polarity and trigger say "conform to the bus".
    pub const fn conforms(self) -> bool {
        self.flags & 0x0f == 0
    }

    pub const fn polarity(self) -> Polarity {
        if self.flags & 0x03 == 0x03 {
            Polarity::ActiveLow
        } else {
            Polarity::ActiveHigh
        }
    }

    pub const fn trigger(self) -> TriggerMode {
        if self.flags & 0x0c == 0x0c {
            TriggerMode::Level
        } else {
            TriggerMode::Edge
        }
    }
}

const EMPTY_IOAPIC: IoApicEntry = IoApicEntry {
    id: 0,
    address: 0,
    gsi_base: 0,
};

const EMPTY_OVERRIDE: IsaOverride = IsaOverride {
    bus: 0,
    source: 0,
    gsi: 0,
    flags: 0,
};

/// The validated subset of a MADT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Madt {
    local_apic_address: u32,
    flags: u32,
    ioapics: [IoApicEntry; MAX_IOAPICS],
    ioapic_count: usize,
    overrides: [IsaOverride; MAX_ISA_OVERRIDES],
    override_count: usize,
}

impl Madt {
    pub const fn local_apic_address(&self) -> u32 {
        self.local_apic_address
    }

    pub const fn flags(&self) -> u32 {
        self.flags
    }

    pub fn ioapics(&self) -> impl Iterator<Item = IoApicEntry> + '_ {
        self.ioapics[..self.ioapic_count].iter().copied()
    }

    pub fn overrides(&self) -> impl Iterator<Item = IsaOverride> + '_ {
        self.overrides[..self.override_count].iter().copied()
    }

    pub const fn ioapic_count(&self) -> usize {
        self.ioapic_count
    }

    pub const fn override_count(&self) -> usize {
        self.override_count
    }

    /// Bus-0 override for `source`, if the firmware supplied one.
    pub fn override_for(&self, source: u8) -> Option<IsaOverride> {
        self.overrides()
            .find(|entry| entry.bus == 0 && entry.source == source)
    }
}

/// Parse an RSDP and validate both checksums when the table is ACPI 2.0+.
pub fn parse_rsdp(bytes: &[u8]) -> Result<Rsdp, AcpiError> {
    if bytes.len() < RSDP_MIN_LEN {
        return Err(AcpiError::Truncated);
    }
    if &bytes[..8] != RSDP_SIGNATURE {
        return Err(AcpiError::BadSignature);
    }
    if !checksum_zero(&bytes[..RSDP_MIN_LEN]) {
        return Err(AcpiError::BadChecksum);
    }

    let revision = bytes[15];
    let rsdt_address = read_u32(bytes, 16).ok_or(AcpiError::Truncated)?;
    let mut xsdt_address = None;

    if revision >= 2 {
        if bytes.len() < RSDP_V2_LEN {
            return Err(AcpiError::Truncated);
        }
        let length = read_u32(bytes, 20).ok_or(AcpiError::Truncated)? as usize;
        if length < RSDP_V2_LEN {
            return Err(AcpiError::BadLength);
        }
        if length > bytes.len() {
            return Err(AcpiError::Truncated);
        }
        if !checksum_zero(&bytes[..length]) {
            return Err(AcpiError::BadChecksum);
        }
        let address = read_u64(bytes, 24).ok_or(AcpiError::Truncated)?;
        if address != 0 {
            xsdt_address = Some(address);
        }
    }

    if rsdt_address == 0 && xsdt_address.is_none() {
        return Err(AcpiError::NoRootTable);
    }

    Ok(Rsdp {
        revision,
        rsdt_address,
        xsdt_address,
    })
}

/// Read the declared length from an SDT prefix without checking its checksum.
///
/// The kernel needs the declared length before it can validate the complete
/// table against the memory map. The full header is still checked afterwards
/// with [`parse_sdt_header`].
pub fn read_sdt_length(bytes: &[u8]) -> Result<u32, AcpiError> {
    if bytes.len() < 8 {
        return Err(AcpiError::Truncated);
    }
    let length = read_u32(bytes, 4).ok_or(AcpiError::Truncated)?;
    if length < SDT_HEADER_LEN as u32 {
        return Err(AcpiError::BadLength);
    }
    Ok(length)
}

/// Parse and checksum a common 36-byte SDT header.
pub fn parse_sdt_header(bytes: &[u8]) -> Result<SdtHeader, AcpiError> {
    if bytes.len() < SDT_HEADER_LEN {
        return Err(AcpiError::Truncated);
    }
    let length = read_u32(bytes, 4).ok_or(AcpiError::Truncated)?;
    if length < SDT_HEADER_LEN as u32 {
        return Err(AcpiError::BadLength);
    }
    if length as usize > bytes.len() {
        return Err(AcpiError::Truncated);
    }
    if !checksum_zero(&bytes[..length as usize]) {
        return Err(AcpiError::BadChecksum);
    }
    let mut signature = [0u8; 4];
    signature.copy_from_slice(&bytes[..4]);
    Ok(SdtHeader {
        signature,
        length,
        revision: bytes[8],
    })
}

/// Parse an XSDT or RSDT after validating its header and entry alignment.
pub fn parse_table_list(bytes: &[u8]) -> Result<TableList<'_>, AcpiError> {
    let header = parse_sdt_header(bytes)?;
    let entry_size = if header.signature == *XSDT_SIGNATURE {
        8
    } else if header.signature == *RSDT_SIGNATURE {
        4
    } else {
        return Err(AcpiError::BadSignature);
    };
    let payload = header.length as usize - SDT_HEADER_LEN;
    if !payload.is_multiple_of(entry_size) {
        return Err(AcpiError::BadLength);
    }
    Ok(TableList {
        bytes: &bytes[..header.length as usize],
        entry_size,
        count: payload / entry_size,
    })
}

/// Parse a MADT, including IOAPIC and bus-0 ISA override entries.
pub fn parse_madt(bytes: &[u8]) -> Result<Madt, AcpiError> {
    let header = parse_sdt_header(bytes)?;
    if header.signature != *MADT_SIGNATURE {
        return Err(AcpiError::BadSignature);
    }
    if header.length < MADT_HEADER_LEN as u32 {
        return Err(AcpiError::BadLength);
    }

    let total = header.length as usize;
    let mut madt = Madt {
        local_apic_address: read_u32(bytes, 36).ok_or(AcpiError::Truncated)?,
        flags: read_u32(bytes, 40).ok_or(AcpiError::Truncated)?,
        ioapics: [EMPTY_IOAPIC; MAX_IOAPICS],
        ioapic_count: 0,
        overrides: [EMPTY_OVERRIDE; MAX_ISA_OVERRIDES],
        override_count: 0,
    };

    let mut offset = MADT_HEADER_LEN;
    while offset < total {
        if total - offset < 2 {
            return Err(AcpiError::Truncated);
        }
        let kind = bytes[offset];
        let length = bytes[offset + 1] as usize;
        if length < 2 {
            return Err(AcpiError::BadEntry);
        }
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= total)
            .ok_or(AcpiError::BadEntry)?;

        match kind {
            1 => {
                if length < 12 {
                    return Err(AcpiError::BadEntry);
                }
                let address = read_u32(bytes, offset + 4).ok_or(AcpiError::BadEntry)?;
                let gsi_base = read_u32(bytes, offset + 8).ok_or(AcpiError::BadEntry)?;
                if address == 0 || !address.is_multiple_of(4096) {
                    return Err(AcpiError::BadAlignment);
                }
                if madt.ioapics().any(|entry| entry.address == address) {
                    return Err(AcpiError::DuplicateIoApic);
                }
                if madt.ioapic_count == MAX_IOAPICS {
                    return Err(AcpiError::TooManyIoApics);
                }
                madt.ioapics[madt.ioapic_count] = IoApicEntry {
                    id: bytes[offset + 2],
                    address,
                    gsi_base,
                };
                madt.ioapic_count += 1;
            }
            2 => {
                if length < 10 {
                    return Err(AcpiError::BadEntry);
                }
                let bus = bytes[offset + 2];
                let source = bytes[offset + 3];
                let gsi = read_u32(bytes, offset + 4).ok_or(AcpiError::BadEntry)?;
                let flags = read_u16(bytes, offset + 8).ok_or(AcpiError::BadEntry)?;
                validate_flags(flags)?;
                if bus == 0 {
                    if madt.overrides().any(|entry| entry.source == source) {
                        return Err(AcpiError::DuplicateOverride);
                    }
                    if madt.override_count == MAX_ISA_OVERRIDES {
                        return Err(AcpiError::TooManyOverrides);
                    }
                    madt.overrides[madt.override_count] = IsaOverride {
                        bus,
                        source,
                        gsi,
                        flags,
                    };
                    madt.override_count += 1;
                }
            }
            _ => {}
        }

        offset = end;
    }

    Ok(madt)
}

fn validate_flags(flags: u16) -> Result<(), AcpiError> {
    let polarity = flags & 0x03;
    let trigger = (flags >> 2) & 0x03;
    if polarity == 2 || trigger == 2 {
        return Err(AcpiError::ReservedFlags);
    }
    Ok(())
}

fn checksum_zero(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let slice = bytes.get(offset..end)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = bytes.get(offset..end)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let slice = bytes.get(offset..end)?;
    Some(u64::from_le_bytes(slice.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    fn set_checksum_upto(bytes: &mut [u8], offset: usize, length: usize) {
        bytes[offset] = 0;
        let sum = bytes[..length]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        bytes[offset] = 0u8.wrapping_sub(sum);
    }

    fn set_checksum(bytes: &mut [u8], offset: usize) {
        set_checksum_upto(bytes, offset, bytes.len());
    }

    fn rsdp_v1(rsdt: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; RSDP_MIN_LEN];
        bytes[..8].copy_from_slice(RSDP_SIGNATURE);
        bytes[15] = 1;
        bytes[16..20].copy_from_slice(&rsdt.to_le_bytes());
        set_checksum(&mut bytes, 8);
        bytes
    }

    fn rsdp_v2(rsdt: u32, xsdt: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; RSDP_V2_LEN];
        bytes[..8].copy_from_slice(RSDP_SIGNATURE);
        bytes[15] = 2;
        bytes[16..20].copy_from_slice(&rsdt.to_le_bytes());
        bytes[20..24].copy_from_slice(&(RSDP_V2_LEN as u32).to_le_bytes());
        bytes[24..32].copy_from_slice(&xsdt.to_le_bytes());
        set_checksum_upto(&mut bytes, 8, RSDP_MIN_LEN);
        set_checksum(&mut bytes, 32);
        bytes
    }

    fn sdt(signature: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let length = SDT_HEADER_LEN + payload.len();
        let mut bytes = vec![0u8; length];
        bytes[..4].copy_from_slice(signature);
        bytes[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        bytes[8] = 1;
        bytes[SDT_HEADER_LEN..].copy_from_slice(payload);
        set_checksum(&mut bytes, 9);
        bytes
    }

    fn table_entries(addresses: &[u64], width: usize) -> Vec<u8> {
        let mut payload = Vec::new();
        for address in addresses {
            payload.extend_from_slice(&address.to_le_bytes()[..width]);
        }
        payload
    }

    fn ioapic_entry(id: u8, address: u32, gsi_base: u32) -> Vec<u8> {
        let mut entry = vec![0u8; 12];
        entry[0] = 1;
        entry[1] = 12;
        entry[2] = id;
        entry[4..8].copy_from_slice(&address.to_le_bytes());
        entry[8..12].copy_from_slice(&gsi_base.to_le_bytes());
        entry
    }

    fn override_entry(bus: u8, source: u8, gsi: u32, flags: u16) -> Vec<u8> {
        let mut entry = vec![0u8; 10];
        entry[0] = 2;
        entry[1] = 10;
        entry[2] = bus;
        entry[3] = source;
        entry[4..8].copy_from_slice(&gsi.to_le_bytes());
        entry[8..10].copy_from_slice(&flags.to_le_bytes());
        entry
    }

    fn madt(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = Vec::new();
        for entry in entries {
            payload.extend_from_slice(entry);
        }
        let length = MADT_HEADER_LEN + payload.len();
        let mut bytes = vec![0u8; length];
        bytes[..4].copy_from_slice(MADT_SIGNATURE);
        bytes[4..8].copy_from_slice(&(length as u32).to_le_bytes());
        bytes[8] = 3;
        bytes[36..40].copy_from_slice(&0xfee0_0000u32.to_le_bytes());
        bytes[40..44].copy_from_slice(&1u32.to_le_bytes());
        bytes[MADT_HEADER_LEN..].copy_from_slice(&payload);
        set_checksum(&mut bytes, 9);
        bytes
    }

    #[test]
    fn reads_sdt_length_from_prefix() {
        let bytes = sdt(b"TEST", &[0u8; 8]);
        assert_eq!(read_sdt_length(&bytes), Ok(bytes.len() as u32));
        assert_eq!(read_sdt_length(&bytes[..7]), Err(AcpiError::Truncated));

        let mut short = bytes;
        short[4..8].copy_from_slice(&35u32.to_le_bytes());
        assert_eq!(read_sdt_length(&short), Err(AcpiError::BadLength));
    }

    #[test]
    fn parses_rsdp_v1_and_v2() {
        let v1 = parse_rsdp(&rsdp_v1(0x1234_5000)).unwrap();
        assert_eq!(v1.revision, 1);
        assert_eq!(v1.rsdt_address, 0x1234_5000);
        assert_eq!(v1.xsdt_address, None);

        let v2 = parse_rsdp(&rsdp_v2(0x1234_5000, 0x5678_9000)).unwrap();
        assert_eq!(v2.revision, 2);
        assert_eq!(v2.rsdt_address, 0x1234_5000);
        assert_eq!(v2.xsdt_address, Some(0x5678_9000));
    }

    #[test]
    fn rsdp_rejects_short_signature_and_checksum() {
        assert_eq!(parse_rsdp(&[0; 19]), Err(AcpiError::Truncated));

        let mut bytes = rsdp_v1(0x1000);
        bytes[0] = b'X';
        assert_eq!(parse_rsdp(&bytes), Err(AcpiError::BadSignature));

        let mut bytes = rsdp_v1(0x1000);
        bytes[9] ^= 1;
        assert_eq!(parse_rsdp(&bytes), Err(AcpiError::BadChecksum));
    }

    #[test]
    fn rsdp_v2_rejects_bad_length_and_extended_checksum() {
        let mut bytes = rsdp_v2(0x1000, 0x2000);
        bytes[20..24].copy_from_slice(&35u32.to_le_bytes());
        assert_eq!(parse_rsdp(&bytes), Err(AcpiError::BadLength));

        let mut bytes = rsdp_v2(0x1000, 0x2000);
        bytes[20..24].copy_from_slice(&40u32.to_le_bytes());
        assert_eq!(parse_rsdp(&bytes), Err(AcpiError::Truncated));

        let mut bytes = rsdp_v2(0x1000, 0x2000);
        bytes[33] ^= 1;
        assert_eq!(parse_rsdp(&bytes), Err(AcpiError::BadChecksum));
    }

    #[test]
    fn rsdp_requires_a_root_table() {
        assert_eq!(parse_rsdp(&rsdp_v1(0)), Err(AcpiError::NoRootTable));

        let mut bytes = rsdp_v2(0, 0);
        set_checksum_upto(&mut bytes, 8, RSDP_MIN_LEN);
        set_checksum(&mut bytes, 32);
        assert_eq!(parse_rsdp(&bytes), Err(AcpiError::NoRootTable));
    }

    #[test]
    fn parses_xsdt_and_rsdt_addresses() {
        let xsdt = sdt(XSDT_SIGNATURE, &table_entries(&[0x1000, 0x2000], 8));
        let list = parse_table_list(&xsdt).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.address(0), Some(0x1000));
        assert_eq!(list.address(1), Some(0x2000));
        assert_eq!(list.address(2), None);

        let rsdt = sdt(RSDT_SIGNATURE, &table_entries(&[0x3000, 0x4000], 4));
        let list = parse_table_list(&rsdt).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.address(0), Some(0x3000));
        assert_eq!(list.address(1), Some(0x4000));
    }

    #[test]
    fn table_list_rejects_bad_header_checksum_and_alignment() {
        let mut xsdt = sdt(XSDT_SIGNATURE, &table_entries(&[0x1000], 8));
        xsdt[10] ^= 1;
        assert_eq!(parse_table_list(&xsdt).unwrap_err(), AcpiError::BadChecksum);

        let mut rsdt = sdt(RSDT_SIGNATURE, &table_entries(&[0x1000], 4));
        rsdt[4..8].copy_from_slice(&41u32.to_le_bytes());
        rsdt.push(0);
        set_checksum(&mut rsdt, 9);
        assert_eq!(parse_table_list(&rsdt).unwrap_err(), AcpiError::BadLength);
    }

    #[test]
    fn table_list_rejects_wrong_signature_and_short_length() {
        let mut bytes = sdt(XSDT_SIGNATURE, &[]);
        bytes[..4].copy_from_slice(b"FACP");
        set_checksum(&mut bytes, 9);
        assert_eq!(
            parse_table_list(&bytes).unwrap_err(),
            AcpiError::BadSignature
        );

        let mut bytes = sdt(XSDT_SIGNATURE, &[]);
        bytes[4..8].copy_from_slice(&35u32.to_le_bytes());
        assert_eq!(parse_table_list(&bytes).unwrap_err(), AcpiError::BadLength);
    }

    #[test]
    fn parses_madt_ioapics_and_overrides() {
        let bytes = madt(&[
            ioapic_entry(0, 0xfec0_0000, 0),
            override_entry(0, 0, 2, 0),
            override_entry(0, 4, 4, 0),
        ]);
        let table = parse_madt(&bytes).unwrap();
        assert_eq!(table.local_apic_address(), 0xfee0_0000);
        assert_eq!(table.flags(), 1);
        assert_eq!(table.ioapic_count(), 1);
        assert_eq!(table.override_count(), 2);

        let ioapic = table.ioapics().next().unwrap();
        assert_eq!(ioapic.id, 0);
        assert_eq!(ioapic.address, 0xfec0_0000);
        assert_eq!(ioapic.gsi_base, 0);

        let timer = table.override_for(0).unwrap();
        assert_eq!(timer.gsi, 2);
        assert!(timer.conforms());
        assert_eq!(timer.trigger(), TriggerMode::Edge);
        assert_eq!(timer.polarity(), Polarity::ActiveHigh);

        let console = table.override_for(4).unwrap();
        assert_eq!(console.gsi, 4);
        assert!(console.conforms());
        assert_eq!(table.override_for(3), None);
    }

    #[test]
    fn decodes_explicit_override_flags_and_ignores_other_buses() {
        let bytes = madt(&[
            ioapic_entry(0, 0xfec0_0000, 0),
            override_entry(0, 1, 9, 0xd),
            override_entry(0, 3, 11, 0x3),
            override_entry(0, 4, 12, 0x1),
            override_entry(0, 5, 13, 0x4),
            override_entry(0, 6, 14, 0xc),
            override_entry(1, 2, 10, 0xd),
        ]);
        let table = parse_madt(&bytes).unwrap();
        assert_eq!(table.override_count(), 5);
        let entry = table.override_for(1).unwrap();
        assert!(!entry.conforms());
        assert_eq!(entry.trigger(), TriggerMode::Level);
        assert_eq!(entry.polarity(), Polarity::ActiveHigh);

        let low = table.override_for(3).unwrap();
        assert_eq!(low.polarity(), Polarity::ActiveLow);
        assert_eq!(low.trigger(), TriggerMode::Edge);

        let high_edge = table.override_for(4).unwrap();
        assert_eq!(high_edge.polarity(), Polarity::ActiveHigh);
        assert_eq!(high_edge.trigger(), TriggerMode::Edge);

        let conforming_polarity = table.override_for(5).unwrap();
        assert_eq!(conforming_polarity.polarity(), Polarity::ActiveHigh);
        assert_eq!(conforming_polarity.trigger(), TriggerMode::Edge);

        let conforming_trigger = table.override_for(6).unwrap();
        assert_eq!(conforming_trigger.polarity(), Polarity::ActiveHigh);
        assert_eq!(conforming_trigger.trigger(), TriggerMode::Level);

        assert_eq!(table.override_for(2), None);
    }

    #[test]
    fn madt_rejects_truncation_and_bad_entries() {
        let mut bytes = madt(&[]);
        bytes[4..8].copy_from_slice(&40u32.to_le_bytes());
        set_checksum_upto(&mut bytes, 9, 40);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::BadLength));

        let bytes = madt(&[vec![1, 1]]);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::BadEntry));

        let bytes = madt(&[vec![1, 13, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]]);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::BadEntry));

        let mut bytes = madt(&[vec![0xff, 2]]);
        bytes[4..8].copy_from_slice(&45u32.to_le_bytes());
        set_checksum_upto(&mut bytes, 9, 45);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::Truncated));
    }

    #[test]
    fn madt_rejects_duplicate_and_misaligned_ioapics() {
        let bytes = madt(&[
            ioapic_entry(0, 0xfec0_0000, 0),
            ioapic_entry(1, 0xfec0_0000, 24),
        ]);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::DuplicateIoApic));

        let bytes = madt(&[ioapic_entry(0, 0xfec0_0800, 0)]);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::BadAlignment));
    }

    #[test]
    fn madt_rejects_duplicate_and_reserved_overrides() {
        let bytes = madt(&[override_entry(0, 0, 2, 0), override_entry(0, 0, 3, 0)]);
        assert_eq!(parse_madt(&bytes), Err(AcpiError::DuplicateOverride));

        for flags in [0x2, 0x8] {
            let bytes = madt(&[override_entry(0, 0, 2, flags)]);
            assert_eq!(parse_madt(&bytes), Err(AcpiError::ReservedFlags));
        }
    }

    #[test]
    fn madt_enforces_capacity() {
        let mut entries = Vec::new();
        for address in 0..MAX_IOAPICS {
            entries.push(ioapic_entry(
                address as u8,
                0xfec0_0000 + (address as u32) * 0x1000,
                0,
            ));
        }
        assert!(parse_madt(&madt(&entries)).is_ok());
        entries.push(ioapic_entry(7, 0xfec1_0000, 0));
        assert_eq!(parse_madt(&madt(&entries)), Err(AcpiError::TooManyIoApics));

        let mut entries = Vec::new();
        for source in 0..MAX_ISA_OVERRIDES {
            entries.push(override_entry(0, source as u8, 0, 0));
        }
        assert!(parse_madt(&madt(&entries)).is_ok());
        entries.push(override_entry(0, 99, 0, 0));
        assert_eq!(
            parse_madt(&madt(&entries)),
            Err(AcpiError::TooManyOverrides)
        );
    }

    #[test]
    fn madt_rejects_bad_checksum() {
        let mut bytes = madt(&[ioapic_entry(0, 0xfec0_0000, 0)]);
        bytes[10] ^= 1;
        assert_eq!(parse_madt(&bytes), Err(AcpiError::BadChecksum));
    }
}
