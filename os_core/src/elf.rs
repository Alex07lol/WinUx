//! Minimal ELF64 `ET_EXEC` parser for the ring-3 program loader.
//!
//! Pure, checked little-endian reads over an in-memory byte slice so every
//! rejection path is host-testable. Only `PT_LOAD` program headers are
//! validated and yielded; other program-header types are ignored. The parser
//! is deliberately address-space agnostic: the user-address window is kernel
//! policy enforced by `kernel/src/loader.rs`.

use core::ops::Range;

use crate::paging::PAGE_SIZE;

pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;
pub const MAX_PROGRAM_HEADERS: u64 = 64;

pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const EI_CLASS_OFFSET: usize = 4;
const EI_DATA_OFFSET: usize = 5;
const EI_VERSION_OFFSET: usize = 6;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const ELF_VERSION: u32 = 1;

/// One `PT_LOAD` program header, validated by [`ElfImage::parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub flags: u32,
}

impl Segment {
    pub fn is_executable(&self) -> bool {
        self.flags & PF_X != 0
    }

    pub fn is_writable(&self) -> bool {
        self.flags & PF_W != 0
    }

    /// Page-rounded `vaddr..vaddr + memsz` coverage. Only meaningful after
    /// `parse` validated the segment, which guarantees no overflow here.
    pub fn page_range(&self) -> Range<u64> {
        self.vaddr..page_round(self.vaddr + self.memsz)
    }
}

/// Every way an image can be rejected; `label()` strings are stable and used
/// verbatim in boot logs and test assertions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    Truncated,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    BadIdentVersion,
    UnsupportedType,
    WrongMachine,
    BadHeaderSizes,
    NoProgramHeaders,
    TooManyProgramHeaders,
    BadSegmentAlignment,
    SegmentSizeMismatch,
    SegmentOutOfFile,
    SegmentRangeOverflow,
    BadSegmentFlags,
    WriteExecuteSegment,
    OverlappingSegments,
    EntryNotExecutable,
}

impl ElfError {
    pub fn label(&self) -> &'static str {
        match self {
            ElfError::Truncated => "truncated",
            ElfError::BadMagic => "bad magic",
            ElfError::Not64Bit => "not 64-bit",
            ElfError::NotLittleEndian => "not little endian",
            ElfError::BadIdentVersion => "bad ident version",
            ElfError::UnsupportedType => "unsupported type",
            ElfError::WrongMachine => "wrong machine",
            ElfError::BadHeaderSizes => "bad header sizes",
            ElfError::NoProgramHeaders => "no program headers",
            ElfError::TooManyProgramHeaders => "too many program headers",
            ElfError::BadSegmentAlignment => "bad segment alignment",
            ElfError::SegmentSizeMismatch => "segment size mismatch",
            ElfError::SegmentOutOfFile => "segment out of file",
            ElfError::SegmentRangeOverflow => "segment range overflow",
            ElfError::BadSegmentFlags => "bad segment flags",
            ElfError::WriteExecuteSegment => "write+execute segment",
            ElfError::OverlappingSegments => "overlapping segments",
            ElfError::EntryNotExecutable => "entry not executable",
        }
    }
}

/// A validated ELF64 `ET_EXEC` image borrowing the original bytes.
#[derive(Clone, Copy, Debug)]
pub struct ElfImage<'a> {
    bytes: &'a [u8],
    entry: u64,
    phoff: u64,
    phnum: u64,
}

impl<'a> ElfImage<'a> {
    /// Validate the whole image up front; nothing is re-checked later.
    pub fn parse(bytes: &'a [u8]) -> Result<ElfImage<'a>, ElfError> {
        if bytes.len() < EHDR_SIZE {
            return Err(ElfError::Truncated);
        }
        if bytes[0..4] != ELF_MAGIC {
            return Err(ElfError::BadMagic);
        }
        if bytes[EI_CLASS_OFFSET] != ELFCLASS64 {
            return Err(ElfError::Not64Bit);
        }
        if bytes[EI_DATA_OFFSET] != ELFDATA2LSB {
            return Err(ElfError::NotLittleEndian);
        }
        if bytes[EI_VERSION_OFFSET] != EV_CURRENT {
            return Err(ElfError::BadIdentVersion);
        }

        let e_type = read_u16(bytes, 16);
        let e_machine = read_u16(bytes, 18);
        let e_version = read_u32(bytes, 20);
        let entry = read_u64(bytes, 24);
        let phoff = read_u64(bytes, 32);
        let ehsize = read_u16(bytes, 52);
        let phentsize = read_u16(bytes, 54);
        let phnum = read_u16(bytes, 56);

        if e_type != ET_EXEC {
            return Err(ElfError::UnsupportedType);
        }
        if e_machine != EM_X86_64 {
            return Err(ElfError::WrongMachine);
        }
        if e_version != ELF_VERSION
            || usize::from(ehsize) != EHDR_SIZE
            || usize::from(phentsize) != PHDR_SIZE
        {
            return Err(ElfError::BadHeaderSizes);
        }
        if phnum == 0 {
            return Err(ElfError::NoProgramHeaders);
        }
        if u64::from(phnum) > MAX_PROGRAM_HEADERS {
            return Err(ElfError::TooManyProgramHeaders);
        }
        let phnum = u64::from(phnum);
        let table_end = phoff
            .checked_add(phnum * PHDR_SIZE as u64)
            .ok_or(ElfError::Truncated)?;
        if table_end > bytes.len() as u64 {
            return Err(ElfError::Truncated);
        }

        let mut segments = [Segment {
            offset: 0,
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            flags: 0,
        }; MAX_PROGRAM_HEADERS as usize];
        let mut count = 0usize;
        for index in 0..phnum {
            let base = (phoff + index * PHDR_SIZE as u64) as usize;
            if read_u32(bytes, base) != PT_LOAD {
                continue;
            }
            let segment = validate_segment(bytes, base)?;
            for previous in &segments[..count] {
                let a = previous.page_range();
                let b = segment.page_range();
                if a.start < b.end && b.start < a.end {
                    return Err(ElfError::OverlappingSegments);
                }
            }
            segments[count] = segment;
            count += 1;
        }

        let entry_ok = segments[..count].iter().any(|segment| {
            segment.is_executable()
                && entry >= segment.vaddr
                && entry < segment.vaddr + segment.memsz
        });
        if !entry_ok {
            return Err(ElfError::EntryNotExecutable);
        }

        Ok(ElfImage {
            bytes,
            entry,
            phoff,
            phnum,
        })
    }

    pub fn entry(&self) -> u64 {
        self.entry
    }

    /// Iterate the already-validated `PT_LOAD` headers.
    pub fn program_headers(&self) -> ProgramHeaders<'a> {
        ProgramHeaders {
            image: *self,
            index: 0,
        }
    }
}

pub struct ProgramHeaders<'a> {
    image: ElfImage<'a>,
    index: u64,
}

impl Iterator for ProgramHeaders<'_> {
    type Item = Segment;

    fn next(&mut self) -> Option<Segment> {
        while self.index < self.image.phnum {
            let base = (self.image.phoff + self.index * PHDR_SIZE as u64) as usize;
            self.index += 1;
            if read_u32(self.image.bytes, base) != PT_LOAD {
                continue;
            }
            return Some(Segment {
                offset: read_u64(self.image.bytes, base + 8),
                vaddr: read_u64(self.image.bytes, base + 16),
                filesz: read_u64(self.image.bytes, base + 32),
                memsz: read_u64(self.image.bytes, base + 40),
                flags: read_u32(self.image.bytes, base + 4),
            });
        }
        None
    }
}

/// Validate one `PT_LOAD` header at `base` (bounds already proven by `parse`).
fn validate_segment(bytes: &[u8], base: usize) -> Result<Segment, ElfError> {
    let flags = read_u32(bytes, base + 4);
    let offset = read_u64(bytes, base + 8);
    let vaddr = read_u64(bytes, base + 16);
    let filesz = read_u64(bytes, base + 32);
    let memsz = read_u64(bytes, base + 40);
    let align = read_u64(bytes, base + 48);

    if align != 0 && align != PAGE_SIZE {
        return Err(ElfError::BadSegmentAlignment);
    }
    if !vaddr.is_multiple_of(PAGE_SIZE) {
        return Err(ElfError::BadSegmentAlignment);
    }
    if filesz > memsz {
        return Err(ElfError::SegmentSizeMismatch);
    }
    let payload_end = offset
        .checked_add(filesz)
        .ok_or(ElfError::SegmentOutOfFile)?;
    if payload_end > bytes.len() as u64 {
        return Err(ElfError::SegmentOutOfFile);
    }
    let end = vaddr
        .checked_add(memsz)
        .ok_or(ElfError::SegmentRangeOverflow)?;
    end.checked_add(PAGE_SIZE - 1)
        .ok_or(ElfError::SegmentRangeOverflow)?;
    if flags & !(PF_R | PF_W | PF_X) != 0 {
        return Err(ElfError::BadSegmentFlags);
    }
    if flags & PF_W != 0 && flags & PF_X != 0 {
        return Err(ElfError::WriteExecuteSegment);
    }

    Ok(Segment {
        offset,
        vaddr,
        filesz,
        memsz,
        flags,
    })
}

fn page_round(end: u64) -> u64 {
    (end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0x40_0000;

    struct RawSegment {
        flags: u32,
        vaddr: u64,
        memsz: u64,
        payload: &'static [u8],
    }

    fn rx(vaddr: u64, payload: &'static [u8]) -> RawSegment {
        RawSegment {
            flags: PF_R | PF_X,
            vaddr,
            memsz: payload.len() as u64,
            payload,
        }
    }

    fn build_image(segments: &[RawSegment], entry: u64) -> Vec<u8> {
        let first_offset = EHDR_SIZE as u64 + segments.len() as u64 * PHDR_SIZE as u64;
        build_image_with(segments, entry, first_offset)
    }

    fn build_image_with(segments: &[RawSegment], entry: u64, first_offset: u64) -> Vec<u8> {
        let phoff = EHDR_SIZE as u64;
        let phnum = segments.len() as u64;
        let mut offsets = Vec::new();
        let mut offset = first_offset;
        for segment in segments {
            offsets.push(offset);
            offset += segment.payload.len() as u64 + 3;
        }
        let end = offset.max(phoff + phnum * PHDR_SIZE as u64);
        let mut image = vec![0u8; end as usize + 16];

        image[0..4].copy_from_slice(&ELF_MAGIC);
        image[EI_CLASS_OFFSET] = ELFCLASS64;
        image[EI_DATA_OFFSET] = ELFDATA2LSB;
        image[EI_VERSION_OFFSET] = EV_CURRENT;
        patch_u16(&mut image, 16, ET_EXEC);
        patch_u16(&mut image, 18, EM_X86_64);
        patch_u32(&mut image, 20, ELF_VERSION);
        patch_u64(&mut image, 24, entry);
        patch_u64(&mut image, 32, phoff);
        patch_u16(&mut image, 52, EHDR_SIZE as u16);
        patch_u16(&mut image, 54, PHDR_SIZE as u16);
        patch_u16(&mut image, 56, phnum as u16);

        for (index, segment) in segments.iter().enumerate() {
            let base = (phoff + index as u64 * PHDR_SIZE as u64) as usize;
            patch_u32(&mut image, base, PT_LOAD);
            patch_u32(&mut image, base + 4, segment.flags);
            patch_u64(&mut image, base + 8, offsets[index]);
            patch_u64(&mut image, base + 16, segment.vaddr);
            patch_u64(&mut image, base + 32, segment.payload.len() as u64);
            patch_u64(&mut image, base + 40, segment.memsz);
            patch_u64(&mut image, base + 48, PAGE_SIZE);
            let start = offsets[index] as usize;
            image[start..start + segment.payload.len()].copy_from_slice(segment.payload);
        }
        image
    }

    fn phdr_base(index: usize) -> usize {
        EHDR_SIZE + index * PHDR_SIZE
    }

    fn patch_u16(image: &mut [u8], offset: usize, value: u16) {
        image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_u32(image: &mut [u8], offset: usize, value: u32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_u64(image: &mut [u8], offset: usize, value: u64) {
        image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn parse(image: &[u8]) -> Result<ElfImage<'_>, ElfError> {
        ElfImage::parse(image)
    }

    #[test]
    fn parses_minimal_executable_image() {
        let image = build_image(&[rx(BASE, b"hello")], BASE);
        let elf = parse(&image).unwrap();
        assert_eq!(elf.entry(), BASE);
        let segments: Vec<_> = elf.program_headers().collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].vaddr, BASE);
        assert_eq!(segments[0].filesz, 5);
        assert_eq!(segments[0].memsz, 5);
        assert!(segments[0].is_executable());
        assert!(!segments[0].is_writable());
        assert_eq!(segments[0].page_range(), BASE..BASE + PAGE_SIZE);
    }

    #[test]
    fn accepts_multi_segment_image_with_distinct_flags() {
        let image = build_image(
            &[
                rx(BASE, b"code"),
                RawSegment {
                    flags: PF_R | PF_W,
                    vaddr: BASE + PAGE_SIZE,
                    memsz: 0x2000,
                    payload: b"data",
                },
            ],
            BASE,
        );
        let elf = parse(&image).unwrap();
        let segments: Vec<_> = elf.program_headers().collect();
        assert_eq!(segments.len(), 2);
        assert!(segments[0].is_executable() && !segments[0].is_writable());
        assert!(segments[1].is_writable() && !segments[1].is_executable());
        // Adjacent page ranges must not count as overlap.
        assert_eq!(segments[1].page_range(), BASE + PAGE_SIZE..BASE + 0x3000);
    }

    #[test]
    fn accepts_unaligned_file_offset() {
        let table_end = EHDR_SIZE as u64 + PHDR_SIZE as u64;
        let image = build_image_with(&[rx(BASE, b"code")], BASE, table_end + 1);
        let elf = parse(&image).unwrap();
        let segment = elf.program_headers().next().unwrap();
        assert_eq!(segment.offset, table_end + 1);
        assert!(!segment.offset.is_multiple_of(PAGE_SIZE));
    }

    #[test]
    fn accepts_bss_only_segment() {
        let image = build_image(
            &[RawSegment {
                flags: PF_R | PF_X,
                vaddr: BASE,
                memsz: PAGE_SIZE,
                payload: b"",
            }],
            BASE,
        );
        let elf = parse(&image).unwrap();
        assert_eq!(
            elf.program_headers().next().unwrap().page_range(),
            BASE..BASE + PAGE_SIZE
        );
    }

    #[test]
    fn rejects_short_image_and_zeroed_magic() {
        assert_eq!(parse(&[0u8; 10]).unwrap_err(), ElfError::Truncated);
        let mut image = build_image(&[rx(BASE, b"code")], BASE);
        image[0..4].fill(0);
        let error = parse(&image).unwrap_err();
        assert_eq!(error, ElfError::BadMagic);
        assert_eq!(error.label(), "bad magic");
    }

    #[test]
    fn rejects_bad_ident_bytes() {
        let valid = build_image(&[rx(BASE, b"code")], BASE);

        let mut not_64 = valid.clone();
        not_64[EI_CLASS_OFFSET] = 1;
        assert_eq!(parse(&not_64).unwrap_err(), ElfError::Not64Bit);

        let mut big_endian = valid.clone();
        big_endian[EI_DATA_OFFSET] = 2;
        assert_eq!(parse(&big_endian).unwrap_err(), ElfError::NotLittleEndian);

        let mut bad_version = valid.clone();
        bad_version[EI_VERSION_OFFSET] = 0;
        assert_eq!(parse(&bad_version).unwrap_err(), ElfError::BadIdentVersion);
    }

    #[test]
    fn rejects_non_exec_type_and_wrong_machine() {
        let mut dynamic = build_image(&[rx(BASE, b"code")], BASE);
        patch_u16(&mut dynamic, 16, 3);
        assert_eq!(parse(&dynamic).unwrap_err(), ElfError::UnsupportedType);

        let mut other_machine = build_image(&[rx(BASE, b"code")], BASE);
        patch_u16(&mut other_machine, 18, 40);
        assert_eq!(parse(&other_machine).unwrap_err(), ElfError::WrongMachine);
    }

    #[test]
    fn rejects_bad_header_sizes() {
        let mut bad_version = build_image(&[rx(BASE, b"code")], BASE);
        patch_u32(&mut bad_version, 20, 2);
        assert_eq!(parse(&bad_version).unwrap_err(), ElfError::BadHeaderSizes);

        let mut bad_ehsize = build_image(&[rx(BASE, b"code")], BASE);
        patch_u16(&mut bad_ehsize, 52, 63);
        assert_eq!(parse(&bad_ehsize).unwrap_err(), ElfError::BadHeaderSizes);

        let mut bad_phentsize = build_image(&[rx(BASE, b"code")], BASE);
        patch_u16(&mut bad_phentsize, 54, 55);
        assert_eq!(parse(&bad_phentsize).unwrap_err(), ElfError::BadHeaderSizes);
    }

    #[test]
    fn rejects_missing_or_excessive_program_headers() {
        let mut none = build_image(&[rx(BASE, b"code")], BASE);
        patch_u16(&mut none, 56, 0);
        assert_eq!(parse(&none).unwrap_err(), ElfError::NoProgramHeaders);

        let mut too_many = build_image(&[rx(BASE, b"code")], BASE);
        patch_u16(&mut too_many, 56, (MAX_PROGRAM_HEADERS + 1) as u16);
        assert_eq!(
            parse(&too_many).unwrap_err(),
            ElfError::TooManyProgramHeaders
        );
    }

    #[test]
    fn rejects_header_table_outside_image() {
        let valid = build_image(&[rx(BASE, b"code")], BASE);

        let mut near_end = valid.clone();
        let near_end_len = near_end.len() as u64;
        patch_u64(&mut near_end, 32, near_end_len - 10);
        assert_eq!(parse(&near_end).unwrap_err(), ElfError::Truncated);

        let mut overflow = valid;
        patch_u64(&mut overflow, 32, u64::MAX);
        assert_eq!(parse(&overflow).unwrap_err(), ElfError::Truncated);
    }

    #[test]
    fn rejects_bad_segment_alignment() {
        let mut misaligned_vaddr = build_image(&[rx(BASE, b"code")], BASE);
        patch_u64(&mut misaligned_vaddr, phdr_base(0) + 16, BASE + 1);
        assert_eq!(
            parse(&misaligned_vaddr).unwrap_err(),
            ElfError::BadSegmentAlignment
        );

        let mut odd_align = build_image(&[rx(BASE, b"code")], BASE);
        patch_u64(&mut odd_align, phdr_base(0) + 48, 2048);
        assert_eq!(
            parse(&odd_align).unwrap_err(),
            ElfError::BadSegmentAlignment
        );
    }

    #[test]
    fn rejects_filesz_greater_than_memsz() {
        let mut image = build_image(&[rx(BASE, b"code")], BASE);
        patch_u64(&mut image, phdr_base(0) + 40, 2);
        assert_eq!(parse(&image).unwrap_err(), ElfError::SegmentSizeMismatch);
    }

    #[test]
    fn rejects_segment_payload_outside_file() {
        let valid = build_image(&[rx(BASE, b"code")], BASE);

        let mut past_end = valid.clone();
        let past_end_len = past_end.len() as u64;
        patch_u64(&mut past_end, phdr_base(0) + 8, past_end_len);
        assert_eq!(parse(&past_end).unwrap_err(), ElfError::SegmentOutOfFile);

        let mut overflow = valid;
        patch_u64(&mut overflow, phdr_base(0) + 8, u64::MAX);
        assert_eq!(parse(&overflow).unwrap_err(), ElfError::SegmentOutOfFile);
    }

    #[test]
    fn rejects_segment_address_range_overflow() {
        let mut end_overflow = build_image(&[rx(0xffff_ffff_ffff_f000, b"code")], BASE);
        patch_u64(&mut end_overflow, 24, 0xffff_ffff_ffff_f000);
        patch_u64(&mut end_overflow, phdr_base(0) + 40, 0x2000);
        assert_eq!(
            parse(&end_overflow).unwrap_err(),
            ElfError::SegmentRangeOverflow
        );

        let mut page_round_overflow = build_image(
            &[RawSegment {
                flags: PF_R | PF_X,
                vaddr: 0xffff_ffff_ffff_f000,
                memsz: 1,
                payload: b"",
            }],
            0xffff_ffff_ffff_f000,
        );
        patch_u64(&mut page_round_overflow, phdr_base(0) + 40, 1);
        assert_eq!(
            parse(&page_round_overflow).unwrap_err(),
            ElfError::SegmentRangeOverflow
        );
    }

    #[test]
    fn rejects_unknown_or_write_execute_flags() {
        let mut unknown = build_image(&[rx(BASE, b"code")], BASE);
        patch_u32(&mut unknown, phdr_base(0) + 4, 8);
        assert_eq!(parse(&unknown).unwrap_err(), ElfError::BadSegmentFlags);

        let mut wx = build_image(&[rx(BASE, b"code")], BASE);
        patch_u32(&mut wx, phdr_base(0) + 4, PF_R | PF_W | PF_X);
        assert_eq!(parse(&wx).unwrap_err(), ElfError::WriteExecuteSegment);
    }

    #[test]
    fn rejects_overlapping_segment_pages() {
        let image = build_image(&[rx(BASE, b"code"), rx(BASE, b"more")], BASE);
        assert_eq!(parse(&image).unwrap_err(), ElfError::OverlappingSegments);
    }

    #[test]
    fn rejects_entry_outside_executable_segments() {
        let past_end = build_image(&[rx(BASE, b"code")], BASE + 5);
        assert_eq!(parse(&past_end).unwrap_err(), ElfError::EntryNotExecutable);

        let in_data = build_image(
            &[
                rx(BASE, b"code"),
                RawSegment {
                    flags: PF_R | PF_W,
                    vaddr: BASE + PAGE_SIZE,
                    memsz: PAGE_SIZE,
                    payload: b"data",
                },
            ],
            BASE + PAGE_SIZE,
        );
        assert_eq!(parse(&in_data).unwrap_err(), ElfError::EntryNotExecutable);

        let empty_exec = build_image(
            &[RawSegment {
                flags: PF_R | PF_X,
                vaddr: BASE,
                memsz: 0,
                payload: b"",
            }],
            BASE,
        );
        assert_eq!(
            parse(&empty_exec).unwrap_err(),
            ElfError::EntryNotExecutable
        );
    }

    #[test]
    fn ignores_non_load_program_headers() {
        let mut image = build_image(&[rx(BASE, b"code"), rx(BASE + PAGE_SIZE, b"more")], BASE);
        patch_u32(&mut image, phdr_base(1), 4);
        let elf = parse(&image).unwrap();
        let segments: Vec<_> = elf.program_headers().collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].vaddr, BASE);
    }

    #[test]
    fn labels_are_stable() {
        let expected = [
            (ElfError::Truncated, "truncated"),
            (ElfError::BadMagic, "bad magic"),
            (ElfError::Not64Bit, "not 64-bit"),
            (ElfError::NotLittleEndian, "not little endian"),
            (ElfError::BadIdentVersion, "bad ident version"),
            (ElfError::UnsupportedType, "unsupported type"),
            (ElfError::WrongMachine, "wrong machine"),
            (ElfError::BadHeaderSizes, "bad header sizes"),
            (ElfError::NoProgramHeaders, "no program headers"),
            (ElfError::TooManyProgramHeaders, "too many program headers"),
            (ElfError::BadSegmentAlignment, "bad segment alignment"),
            (ElfError::SegmentSizeMismatch, "segment size mismatch"),
            (ElfError::SegmentOutOfFile, "segment out of file"),
            (ElfError::SegmentRangeOverflow, "segment range overflow"),
            (ElfError::BadSegmentFlags, "bad segment flags"),
            (ElfError::WriteExecuteSegment, "write+execute segment"),
            (ElfError::OverlappingSegments, "overlapping segments"),
            (ElfError::EntryNotExecutable, "entry not executable"),
        ];
        for (error, label) in expected {
            assert_eq!(error.label(), label);
        }
    }
}
