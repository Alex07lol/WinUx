//! Minimal PE32+ (`IMAGE_FILE_MACHINE_AMD64`) parser for the ring-3 PE loader.
//!
//! Pure, checked little-endian reads over an in-memory byte slice so every
//! rejection path is host-testable. Only the DOS header, COFF header, PE32+
//! optional header, section table and base-relocation directory are
//! validated and yielded; anything else is either rejected by policy
//! (imports, delay imports, TLS, CLR) or ignored (exports, resources,
//! exception data, debug data). The parser is deliberately address-space
//! agnostic: the fixed load window, W^X policy and relocation *application*
//! live in the kernel loader (`kernel/src/pe_loader.rs`).

use core::ops::Range;

pub const DOS_HEADER_SIZE: usize = 0x40;
pub const PE_SIGNATURE: [u8; 4] = *b"PE\0\0";

const COFF_HEADER_SIZE: usize = 20;
const SECTION_HEADER_SIZE: usize = 40;
const OPTIONAL_MIN_SIZE: usize = 112;
const OPTIONAL_MAGIC_PE32PLUS: u16 = 0x20B;

const E_LFANEW_OFFSET: usize = 0x3C;
const OPT_MAGIC_OFFSET: usize = 0;
const OPT_ENTRY_OFFSET: usize = 16;
const OPT_IMAGE_BASE_OFFSET: usize = 24;
const OPT_SECTION_ALIGN_OFFSET: usize = 32;
const OPT_FILE_ALIGN_OFFSET: usize = 36;
const OPT_SIZE_OF_IMAGE_OFFSET: usize = 56;
const OPT_SUBSYSTEM_OFFSET: usize = 68;
const OPT_NUMBER_OF_DIRS_OFFSET: usize = 108;
const OPT_DIRECTORIES_OFFSET: usize = 112;

pub const MAX_SECTIONS: usize = 32;
pub const MAX_RELOCATIONS: usize = 256;

pub const IMAGE_FILE_RELOCS_STRIPPED: u16 = 0x0001;
pub const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
pub const IMAGE_FILE_DLL: u16 = 0x2000;
pub const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
pub const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;

pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

const IMAGE_DIRECTORY_ENTRY_IMPORT: u32 = 1;
const IMAGE_DIRECTORY_ENTRY_BASERELOC: u32 = 5;
const IMAGE_DIRECTORY_ENTRY_TLS: u32 = 9;
const IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT: u32 = 13;
const IMAGE_DIRECTORY_ENTRY_COM_DESCRIPTOR: u32 = 14;

const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_DIR64: u16 = 10;

/// One section header, validated by [`PeImage::parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 8],
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

impl Section {
    pub const EMPTY: Section = Section {
        name: [0; 8],
        virtual_address: 0,
        virtual_size: 0,
        raw_offset: 0,
        raw_size: 0,
        characteristics: 0,
    };

    pub fn is_executable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_EXECUTE != 0
    }

    pub fn is_writable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_WRITE != 0
    }

    pub fn is_readable(&self) -> bool {
        self.characteristics & IMAGE_SCN_MEM_READ != 0
    }

    /// `max(VirtualSize, SizeOfRawData)`: the mapped size of the section.
    pub fn memory_size(&self) -> u64 {
        u64::from(self.virtual_size.max(self.raw_size))
    }

    /// `virtual_address .. virtual_address + memory_size`; only meaningful
    /// after `parse` validated the section, which guarantees no overflow.
    pub fn rva_range(&self) -> Range<u64> {
        let start = u64::from(self.virtual_address);
        start..start + self.memory_size()
    }
}

/// Every way an image can be rejected; `label()` strings are stable and used
/// verbatim in boot logs and test assertions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeError {
    Truncated,
    BadDosMagic,
    BadSignature,
    NotPE32Plus,
    WrongMachine,
    NotExecutable,
    IsDll,
    BadOptionalHeader,
    BadDataDirectories,
    BadFileAlignment,
    BadSectionAlignment,
    BadSizeOfImage,
    NoSections,
    TooManySections,
    SectionTableOutOfFile,
    BadSectionLayout,
    SectionOutOfFile,
    EntryNotExecutable,
    UnsupportedSubsystem,
    ImportsUnsupported,
    DelayImportsUnsupported,
    TlsUnsupported,
    ClrUnsupported,
    BadRelocationTable,
    UnsupportedRelocationType,
    TooManyRelocations,
}

impl PeError {
    pub fn label(&self) -> &'static str {
        match self {
            PeError::Truncated => "truncated",
            PeError::BadDosMagic => "bad dos magic",
            PeError::BadSignature => "bad signature",
            PeError::NotPE32Plus => "not PE32+",
            PeError::WrongMachine => "wrong machine",
            PeError::NotExecutable => "not an executable image",
            PeError::IsDll => "is a dll",
            PeError::BadOptionalHeader => "bad optional header",
            PeError::BadDataDirectories => "bad data directories",
            PeError::BadFileAlignment => "bad file alignment",
            PeError::BadSectionAlignment => "bad section alignment",
            PeError::BadSizeOfImage => "bad size of image",
            PeError::NoSections => "no sections",
            PeError::TooManySections => "too many sections",
            PeError::SectionTableOutOfFile => "section table out of file",
            PeError::BadSectionLayout => "bad section layout",
            PeError::SectionOutOfFile => "section out of file",
            PeError::EntryNotExecutable => "entry not executable",
            PeError::UnsupportedSubsystem => "unsupported subsystem",
            PeError::ImportsUnsupported => "imports unsupported",
            PeError::DelayImportsUnsupported => "delay imports unsupported",
            PeError::TlsUnsupported => "tls unsupported",
            PeError::ClrUnsupported => "clr unsupported",
            PeError::BadRelocationTable => "bad relocation table",
            PeError::UnsupportedRelocationType => "unsupported relocation type",
            PeError::TooManyRelocations => "too many relocations",
        }
    }
}

/// A validated PE32+ image borrowing the original bytes.
#[derive(Debug)]
pub struct PeImage<'a> {
    bytes: &'a [u8],
    image_base: u64,
    entry_rva: u32,
    size_of_image: u32,
    relocation_stripped: bool,
    sections: [Section; MAX_SECTIONS],
    section_count: usize,
    relocations: [u32; MAX_RELOCATIONS],
    relocation_count: usize,
}

impl<'a> PeImage<'a> {
    /// Validate the whole image up front; nothing is re-checked later.
    pub fn parse(bytes: &'a [u8]) -> Result<PeImage<'a>, PeError> {
        if bytes.len() < DOS_HEADER_SIZE {
            return Err(PeError::Truncated);
        }
        if bytes[0..2] != *b"MZ" {
            return Err(PeError::BadDosMagic);
        }

        let pe_offset = read_u32(bytes, E_LFANEW_OFFSET) as usize;
        let signature_end = pe_offset.checked_add(PE_SIGNATURE.len());
        match signature_end {
            Some(end) if end <= bytes.len() => {}
            _ => return Err(PeError::BadSignature),
        }
        if bytes[pe_offset..pe_offset + 4] != PE_SIGNATURE {
            return Err(PeError::BadSignature);
        }

        let coff = pe_offset + 4;
        if coff + COFF_HEADER_SIZE > bytes.len() {
            return Err(PeError::Truncated);
        }
        let machine = read_u16(bytes, coff);
        let num_sections = read_u16(bytes, coff + 2);
        let size_optional = read_u16(bytes, coff + 16) as usize;
        let characteristics = read_u16(bytes, coff + 18);

        if machine != IMAGE_FILE_MACHINE_AMD64 {
            return Err(PeError::WrongMachine);
        }
        if characteristics & IMAGE_FILE_EXECUTABLE_IMAGE == 0 {
            return Err(PeError::NotExecutable);
        }
        if characteristics & IMAGE_FILE_DLL != 0 {
            return Err(PeError::IsDll);
        }

        let optional = coff + COFF_HEADER_SIZE;
        let optional_end = optional
            .checked_add(size_optional)
            .ok_or(PeError::Truncated)?;
        if optional_end > bytes.len() {
            return Err(PeError::Truncated);
        }
        if size_optional < OPTIONAL_MIN_SIZE {
            return Err(PeError::BadOptionalHeader);
        }
        if read_u16(bytes, optional + OPT_MAGIC_OFFSET) != OPTIONAL_MAGIC_PE32PLUS {
            return Err(PeError::NotPE32Plus);
        }

        let entry_rva = read_u32(bytes, optional + OPT_ENTRY_OFFSET);
        let image_base = read_u64(bytes, optional + OPT_IMAGE_BASE_OFFSET);
        let section_align = read_u32(bytes, optional + OPT_SECTION_ALIGN_OFFSET);
        let file_align = read_u32(bytes, optional + OPT_FILE_ALIGN_OFFSET);
        let size_of_image = read_u32(bytes, optional + OPT_SIZE_OF_IMAGE_OFFSET);
        let subsystem = read_u16(bytes, optional + OPT_SUBSYSTEM_OFFSET);
        let num_dirs = read_u32(bytes, optional + OPT_NUMBER_OF_DIRS_OFFSET);

        if num_dirs < IMAGE_DIRECTORY_ENTRY_BASERELOC + 1 {
            return Err(PeError::BadDataDirectories);
        }
        if OPTIONAL_MIN_SIZE as u64 + u64::from(num_dirs) * 8 > size_optional as u64 {
            return Err(PeError::BadOptionalHeader);
        }

        if !file_align.is_power_of_two() || !(512..=65536).contains(&file_align) {
            return Err(PeError::BadFileAlignment);
        }
        if !section_align.is_power_of_two() || section_align < file_align {
            return Err(PeError::BadSectionAlignment);
        }
        if size_of_image == 0 || !size_of_image.is_multiple_of(section_align) {
            return Err(PeError::BadSizeOfImage);
        }

        if num_sections == 0 {
            return Err(PeError::NoSections);
        }
        if usize::from(num_sections) > MAX_SECTIONS {
            return Err(PeError::TooManySections);
        }
        let num_sections = usize::from(num_sections);

        let table = optional_end;
        let table_end = table
            .checked_add(num_sections * SECTION_HEADER_SIZE)
            .ok_or(PeError::SectionTableOutOfFile)?;
        if table_end > bytes.len() {
            return Err(PeError::SectionTableOutOfFile);
        }

        let mut sections = [Section::EMPTY; MAX_SECTIONS];
        for index in 0..num_sections {
            let base = table + index * SECTION_HEADER_SIZE;
            let mut name = [0u8; 8];
            name.copy_from_slice(&bytes[base..base + 8]);
            let section = Section {
                name,
                virtual_size: read_u32(bytes, base + 8),
                virtual_address: read_u32(bytes, base + 12),
                raw_size: read_u32(bytes, base + 16),
                raw_offset: read_u32(bytes, base + 20),
                characteristics: read_u32(bytes, base + 36),
            };
            validate_section(
                &section,
                section_align,
                file_align,
                size_of_image,
                bytes.len(),
            )?;
            if index > 0 {
                let previous = &sections[index - 1];
                let previous_end = u64::from(previous.virtual_address)
                    + round_up(previous.memory_size(), u64::from(section_align));
                if u64::from(section.virtual_address) < previous_end {
                    return Err(PeError::BadSectionLayout);
                }
            }
            sections[index] = section;
        }
        let sections = &sections[..num_sections];

        let entry_ok = sections.iter().any(|section| {
            section.is_executable()
                && u64::from(entry_rva) >= u64::from(section.virtual_address)
                && u64::from(entry_rva) < section.rva_range().end
        });
        if !entry_ok {
            return Err(PeError::EntryNotExecutable);
        }

        if subsystem != IMAGE_SUBSYSTEM_WINDOWS_CUI {
            return Err(PeError::UnsupportedSubsystem);
        }

        if is_nonempty(bytes, optional, num_dirs, IMAGE_DIRECTORY_ENTRY_IMPORT) {
            return Err(PeError::ImportsUnsupported);
        }
        if is_nonempty(
            bytes,
            optional,
            num_dirs,
            IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT,
        ) {
            return Err(PeError::DelayImportsUnsupported);
        }
        if is_nonempty(bytes, optional, num_dirs, IMAGE_DIRECTORY_ENTRY_TLS) {
            return Err(PeError::TlsUnsupported);
        }
        if is_nonempty(
            bytes,
            optional,
            num_dirs,
            IMAGE_DIRECTORY_ENTRY_COM_DESCRIPTOR,
        ) {
            return Err(PeError::ClrUnsupported);
        }

        let mut relocations = [0u32; MAX_RELOCATIONS];
        let relocation_count =
            parse_relocations(bytes, sections, optional, num_dirs, &mut relocations)?;

        Ok(PeImage {
            bytes,
            image_base,
            entry_rva,
            size_of_image,
            relocation_stripped: characteristics & IMAGE_FILE_RELOCS_STRIPPED != 0,
            sections: {
                let mut copy = [Section::EMPTY; MAX_SECTIONS];
                copy[..num_sections].copy_from_slice(sections);
                copy
            },
            section_count: num_sections,
            relocations,
            relocation_count,
        })
    }

    /// The original image bytes (sections carry raw offsets into this).
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    pub fn image_base(&self) -> u64 {
        self.image_base
    }

    pub fn entry_rva(&self) -> u32 {
        self.entry_rva
    }

    pub fn size_of_image(&self) -> u32 {
        self.size_of_image
    }

    pub fn relocation_stripped(&self) -> bool {
        self.relocation_stripped
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections[..self.section_count]
    }

    /// DIR64 target RVAs in table order; `ABSOLUTE` padding is skipped.
    pub fn relocations(&self) -> &[u32] {
        &self.relocations[..self.relocation_count]
    }
}

/// Exact `raw_size == 0` convention: no raw data means no file offset.
fn validate_section(
    section: &Section,
    section_align: u32,
    file_align: u32,
    size_of_image: u32,
    file_len: usize,
) -> Result<(), PeError> {
    if !section.virtual_address.is_multiple_of(section_align) {
        return Err(PeError::BadSectionLayout);
    }
    if section.memory_size() == 0 {
        return Err(PeError::BadSectionLayout);
    }
    if section.raw_size > 0 {
        if section.raw_offset == 0
            || !section.raw_offset.is_multiple_of(file_align)
            || !section.raw_size.is_multiple_of(file_align)
        {
            return Err(PeError::BadSectionLayout);
        }
        let raw_end = u64::from(section.raw_offset) + u64::from(section.raw_size);
        if raw_end > file_len as u64 {
            return Err(PeError::SectionOutOfFile);
        }
    } else if section.raw_offset != 0 {
        return Err(PeError::BadSectionLayout);
    }
    let memory_end = u64::from(section.virtual_address) + section.memory_size();
    if memory_end > u64::from(size_of_image) {
        return Err(PeError::BadSectionLayout);
    }
    Ok(())
}

fn is_nonempty(bytes: &[u8], optional: usize, num_dirs: u32, index: u32) -> bool {
    if index >= num_dirs {
        return false;
    }
    let base = optional + OPT_DIRECTORIES_OFFSET + index as usize * 8;
    read_u32(bytes, base) != 0 || read_u32(bytes, base + 4) != 0
}

/// Walk the base-relocation directory and copy DIR64 target RVAs.
///
/// The directory is an RVA into a section's raw bytes; every block is fully
/// validated before a single entry is read.
fn parse_relocations(
    bytes: &[u8],
    sections: &[Section],
    optional: usize,
    num_dirs: u32,
    relocations: &mut [u32; MAX_RELOCATIONS],
) -> Result<usize, PeError> {
    if IMAGE_DIRECTORY_ENTRY_BASERELOC >= num_dirs {
        return Ok(0);
    }
    let dir = optional + OPT_DIRECTORIES_OFFSET + IMAGE_DIRECTORY_ENTRY_BASERELOC as usize * 8;
    let directory_rva = read_u32(bytes, dir);
    let directory_size = read_u32(bytes, dir + 4) as usize;
    if directory_rva == 0 || directory_size == 0 {
        return Ok(0);
    }

    let start = rva_to_raw_offset(sections, directory_rva, directory_size)
        .ok_or(PeError::BadRelocationTable)?;
    let end = start + directory_size;

    let mut count = 0usize;
    let mut cursor = start;
    while cursor < end {
        if cursor + 8 > end {
            return Err(PeError::BadRelocationTable);
        }
        let page_rva = read_u32(bytes, cursor);
        let block_size = read_u32(bytes, cursor + 4) as usize;
        if block_size < 8
            || !block_size.is_multiple_of(2)
            || cursor
                .checked_add(block_size)
                .is_none_or(|block_end| block_end > end)
        {
            return Err(PeError::BadRelocationTable);
        }
        let entries = (block_size - 8) / 2;
        for entry_index in 0..entries {
            let entry = read_u16(bytes, cursor + 8 + entry_index * 2);
            let kind = entry >> 12;
            let offset = u32::from(entry & 0x0FFF);
            match kind {
                IMAGE_REL_BASED_ABSOLUTE => {}
                IMAGE_REL_BASED_DIR64 => {
                    let target = page_rva
                        .checked_add(offset)
                        .ok_or(PeError::BadRelocationTable)?;
                    if count == MAX_RELOCATIONS {
                        return Err(PeError::TooManyRelocations);
                    }
                    relocations[count] = target;
                    count += 1;
                }
                _ => return Err(PeError::UnsupportedRelocationType),
            }
        }
        cursor += block_size;
    }
    Ok(count)
}

/// Translate an RVA range into a file offset, requiring it to live entirely
/// inside one section's raw data.
fn rva_to_raw_offset(sections: &[Section], rva: u32, size: usize) -> Option<usize> {
    for section in sections {
        if section.raw_size == 0 {
            continue;
        }
        let start = u64::from(section.virtual_address);
        let end = start + u64::from(section.raw_size);
        let rva = u64::from(rva);
        if rva >= start && rva < end && rva + size as u64 <= end {
            return Some((u64::from(section.raw_offset) + (rva - start)) as usize);
        }
    }
    None
}

fn round_up(value: u64, align: u64) -> u64 {
    (value + align - 1) & !(align - 1)
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

    const PE_OFFSET: usize = 0x80;
    const COFF: usize = PE_OFFSET + 4;
    const OPTIONAL: usize = COFF + COFF_HEADER_SIZE;
    const OPTIONAL_SIZE: usize = 0xF0;
    const TABLE: usize = OPTIONAL + OPTIONAL_SIZE;
    const TEXT_RAW: usize = 0x200;
    const RDATA_RAW: usize = 0x400;
    const DATA_RAW: usize = 0x600;
    const RELOC_RAW: usize = 0x800;
    const RELOC_RVA: u32 = 0x4000;
    const IMAGE_BASE: u64 = 0x1_4000_0000;
    const SIZE_OF_IMAGE: u32 = 0x6000;

    fn patch_u16(image: &mut [u8], offset: usize, value: u16) {
        image[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_u32(image: &mut [u8], offset: usize, value: u32) {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_u64(image: &mut [u8], offset: usize, value: u64) {
        image[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn patch_dir(image: &mut [u8], index: usize, rva: u32, size: u32) {
        patch_u32(image, OPTIONAL + OPT_DIRECTORIES_OFFSET + index * 8, rva);
        patch_u32(
            image,
            OPTIONAL + OPT_DIRECTORIES_OFFSET + index * 8 + 4,
            size,
        );
    }

    fn section_base(index: usize) -> usize {
        TABLE + index * SECTION_HEADER_SIZE
    }

    fn patch_section(
        image: &mut [u8],
        index: usize,
        virtual_address: u32,
        virtual_size: u32,
        raw_offset: u32,
        raw_size: u32,
    ) {
        let base = section_base(index);
        patch_u32(image, base + 8, virtual_size);
        patch_u32(image, base + 12, virtual_address);
        patch_u32(image, base + 16, raw_size);
        patch_u32(image, base + 20, raw_offset);
    }

    /// One `.text` RX, `.rdata` R, `.data` RW, `.reloc` R section, a single
    /// DIR64 relocation to `.data` RVA 0x3000, entry 0x1000.
    fn base_image() -> Vec<u8> {
        let mut image = vec![0u8; 0xC00];
        patch_u16(&mut image, 0, 0x5A4D); // "MZ"
        patch_u32(&mut image, E_LFANEW_OFFSET, PE_OFFSET as u32);
        image[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(&PE_SIGNATURE);

        patch_u16(&mut image, COFF, IMAGE_FILE_MACHINE_AMD64);
        patch_u16(&mut image, COFF + 2, 4);
        patch_u16(&mut image, COFF + 16, OPTIONAL_SIZE as u16);
        patch_u16(&mut image, COFF + 18, IMAGE_FILE_EXECUTABLE_IMAGE | 0x0020);

        patch_u16(
            &mut image,
            OPTIONAL + OPT_MAGIC_OFFSET,
            OPTIONAL_MAGIC_PE32PLUS,
        );
        patch_u32(&mut image, OPTIONAL + OPT_ENTRY_OFFSET, 0x1000);
        patch_u64(&mut image, OPTIONAL + OPT_IMAGE_BASE_OFFSET, IMAGE_BASE);
        patch_u32(&mut image, OPTIONAL + OPT_SECTION_ALIGN_OFFSET, 0x1000);
        patch_u32(&mut image, OPTIONAL + OPT_FILE_ALIGN_OFFSET, 0x200);
        patch_u32(
            &mut image,
            OPTIONAL + OPT_SIZE_OF_IMAGE_OFFSET,
            SIZE_OF_IMAGE,
        );
        patch_u32(&mut image, OPTIONAL + 60, 0x200);
        patch_u16(
            &mut image,
            OPTIONAL + OPT_SUBSYSTEM_OFFSET,
            IMAGE_SUBSYSTEM_WINDOWS_CUI,
        );
        patch_u32(&mut image, OPTIONAL + OPT_NUMBER_OF_DIRS_OFFSET, 16);

        struct RawSection {
            name: &'static [u8; 8],
            va: u32,
            vsize: u32,
            raw_offset: u32,
            raw_size: u32,
            characteristics: u32,
        }
        let sections = [
            RawSection {
                name: b".text\0\0\0",
                va: 0x1000,
                vsize: 0x200,
                raw_offset: TEXT_RAW as u32,
                raw_size: 0x200,
                characteristics: 0x6000_0020,
            },
            RawSection {
                name: b".rdata\0\0",
                va: 0x2000,
                vsize: 0x200,
                raw_offset: RDATA_RAW as u32,
                raw_size: 0x200,
                characteristics: 0x4000_0040,
            },
            RawSection {
                name: b".data\0\0\0",
                va: 0x3000,
                vsize: 0x200,
                raw_offset: DATA_RAW as u32,
                raw_size: 0x200,
                characteristics: 0xC000_0040,
            },
            RawSection {
                name: b".reloc\0\0",
                va: RELOC_RVA,
                vsize: 0x400,
                raw_offset: RELOC_RAW as u32,
                raw_size: 0x400,
                characteristics: 0x4200_0040,
            },
        ];
        for (index, section) in sections.iter().enumerate() {
            let base = section_base(index);
            image[base..base + 8].copy_from_slice(section.name);
            patch_section(
                &mut image,
                index,
                section.va,
                section.vsize,
                section.raw_offset,
                section.raw_size,
            );
            patch_u32(&mut image, base + 36, section.characteristics);
        }

        write_relocations(&mut image, &[(0x3000, &[10 << 12])]);
        image
    }

    /// Write base-relocation blocks at the `.reloc` raw offset and point the
    /// directory at them.
    fn write_relocations(image: &mut [u8], blocks: &[(u32, &[u16])]) {
        let mut offset = RELOC_RAW;
        for (page_rva, entries) in blocks {
            patch_u32(image, offset, *page_rva);
            patch_u32(image, offset + 4, (8 + entries.len() * 2) as u32);
            for (index, entry) in entries.iter().enumerate() {
                patch_u16(image, offset + 8 + index * 2, *entry);
            }
            offset += 8 + entries.len() * 2;
        }
        patch_dir(
            image,
            IMAGE_DIRECTORY_ENTRY_BASERELOC as usize,
            RELOC_RVA,
            (offset - RELOC_RAW) as u32,
        );
    }

    fn parse(image: &[u8]) -> Result<PeImage<'_>, PeError> {
        PeImage::parse(image)
    }

    fn assert_rejects(image: &[u8], expected: PeError) {
        assert_eq!(parse(image).unwrap_err(), expected);
    }

    #[test]
    fn parses_minimal_image() {
        let image = base_image();
        let pe = parse(&image).unwrap();
        assert_eq!(pe.image_base(), IMAGE_BASE);
        assert_eq!(pe.entry_rva(), 0x1000);
        assert_eq!(pe.size_of_image(), SIZE_OF_IMAGE);
        assert!(!pe.relocation_stripped());
        assert_eq!(pe.sections().len(), 4);
        assert_eq!(&pe.sections()[0].name, b".text\0\0\0");
        assert_eq!(pe.sections()[3].virtual_address, RELOC_RVA);
        assert_eq!(pe.relocations(), &[0x3000]);
    }

    #[test]
    fn reports_section_permissions() {
        let image = base_image();
        let pe = parse(&image).unwrap();
        let sections = pe.sections();
        assert!(sections[0].is_executable() && sections[0].is_readable());
        assert!(!sections[0].is_writable());
        assert!(sections[1].is_readable() && !sections[1].is_executable());
        assert!(sections[2].is_writable() && !sections[2].is_executable());
        assert_eq!(sections[0].memory_size(), 0x200);
        assert_eq!(sections[0].rva_range(), 0x1000..0x1200);
    }

    #[test]
    fn accepts_virtual_size_larger_than_raw_size() {
        let mut image = base_image();
        patch_section(&mut image, 1, 0x2000, 0x300, RDATA_RAW as u32, 0x200);
        let pe = parse(&image).unwrap();
        assert_eq!(pe.sections()[1].memory_size(), 0x300);
    }

    #[test]
    fn accepts_raw_size_larger_than_virtual_size() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0x100, DATA_RAW as u32, 0x200);
        let pe = parse(&image).unwrap();
        assert_eq!(pe.sections()[2].memory_size(), 0x200);
    }

    #[test]
    fn skips_absolute_relocation_padding() {
        let mut image = base_image();
        write_relocations(&mut image, &[(0x3000, &[0, 10 << 12, 0, (10 << 12) | 8])]);
        let pe = parse(&image).unwrap();
        assert_eq!(pe.relocations(), &[0x3000, 0x3008]);
    }

    #[test]
    fn accepts_multiple_relocation_blocks() {
        let mut image = base_image();
        write_relocations(
            &mut image,
            &[(0x3000, &[10 << 12]), (0x1000, &[0, (10 << 12) | 4])],
        );
        let pe = parse(&image).unwrap();
        assert_eq!(pe.relocations(), &[0x3000, 0x1004]);
    }

    #[test]
    fn accepts_empty_relocation_directory() {
        let mut image = base_image();
        patch_dir(&mut image, IMAGE_DIRECTORY_ENTRY_BASERELOC as usize, 0, 0);
        let pe = parse(&image).unwrap();
        assert!(pe.relocations().is_empty());
    }

    #[test]
    fn exposes_stripped_relocations_flag_without_walking_policy() {
        let mut image = base_image();
        let characteristics = IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_RELOCS_STRIPPED | 0x0020;
        patch_u16(&mut image, COFF + 18, characteristics);
        let pe = parse(&image).unwrap();
        assert!(pe.relocation_stripped());
        assert_eq!(pe.relocations(), &[0x3000]);
    }

    #[test]
    fn rejects_short_file() {
        assert_rejects(&[0u8; DOS_HEADER_SIZE - 1], PeError::Truncated);
    }

    #[test]
    fn rejects_bad_dos_magic() {
        let mut image = base_image();
        image[0] = b'X';
        assert_rejects(&image, PeError::BadDosMagic);
    }

    #[test]
    fn rejects_zeroed_signature() {
        let mut image = base_image();
        image[PE_OFFSET..PE_OFFSET + 4].fill(0);
        assert_rejects(&image, PeError::BadSignature);
    }

    #[test]
    fn rejects_e_lfanew_beyond_file() {
        let mut image = base_image();
        patch_u32(&mut image, E_LFANEW_OFFSET, 0x2000);
        assert_rejects(&image, PeError::BadSignature);
    }

    #[test]
    fn rejects_e_lfanew_near_end() {
        let mut image = base_image();
        let near_end = image.len() as u32 - 2;
        patch_u32(&mut image, E_LFANEW_OFFSET, near_end);
        assert_rejects(&image, PeError::BadSignature);
    }

    #[test]
    fn rejects_e_lfanew_inside_file_but_past_headers() {
        let mut image = base_image();
        patch_u32(&mut image, E_LFANEW_OFFSET, 0x400);
        assert_rejects(&image, PeError::BadSignature);
    }

    #[test]
    fn rejects_truncated_after_signature() {
        let mut image = base_image();
        let len = COFF + 4;
        image.truncate(len);
        assert_rejects(&image, PeError::Truncated);
    }

    #[test]
    fn rejects_not_pe32_plus() {
        let mut image = base_image();
        patch_u16(&mut image, OPTIONAL + OPT_MAGIC_OFFSET, 0x10B);
        assert_rejects(&image, PeError::NotPE32Plus);
    }

    #[test]
    fn rejects_wrong_machine() {
        let mut image = base_image();
        patch_u16(&mut image, COFF, 0x014C);
        assert_rejects(&image, PeError::WrongMachine);
    }

    #[test]
    fn rejects_non_executable_image() {
        let mut image = base_image();
        patch_u16(&mut image, COFF + 18, 0x0020);
        assert_rejects(&image, PeError::NotExecutable);
    }

    #[test]
    fn rejects_dll() {
        let mut image = base_image();
        patch_u16(
            &mut image,
            COFF + 18,
            IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_DLL,
        );
        assert_rejects(&image, PeError::IsDll);
    }

    #[test]
    fn rejects_small_optional_header() {
        let mut image = base_image();
        patch_u16(&mut image, COFF + 16, 96);
        assert_rejects(&image, PeError::BadOptionalHeader);
    }

    #[test]
    fn rejects_optional_header_past_file() {
        let mut image = base_image();
        image.truncate(OPTIONAL + 0x40);
        assert_rejects(&image, PeError::Truncated);
    }

    #[test]
    fn rejects_directories_not_fitting_optional_header() {
        let mut image = base_image();
        patch_u16(&mut image, COFF + 16, 0x70);
        assert_rejects(&image, PeError::BadOptionalHeader);
    }

    #[test]
    fn rejects_missing_data_directories() {
        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_NUMBER_OF_DIRS_OFFSET, 5);
        assert_rejects(&image, PeError::BadDataDirectories);
    }

    #[test]
    fn rejects_bad_file_alignment() {
        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_FILE_ALIGN_OFFSET, 0x300);
        assert_rejects(&image, PeError::BadFileAlignment);

        patch_u32(&mut image, OPTIONAL + OPT_FILE_ALIGN_OFFSET, 0x100);
        assert_rejects(&image, PeError::BadFileAlignment);
    }

    #[test]
    fn rejects_bad_section_alignment() {
        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_SECTION_ALIGN_OFFSET, 0x100);
        assert_rejects(&image, PeError::BadSectionAlignment);

        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_SECTION_ALIGN_OFFSET, 0x3000);
        assert_rejects(&image, PeError::BadSectionAlignment);
    }

    #[test]
    fn rejects_bad_size_of_image() {
        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_SIZE_OF_IMAGE_OFFSET, 0);
        assert_rejects(&image, PeError::BadSizeOfImage);

        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_SIZE_OF_IMAGE_OFFSET, 0x5500);
        assert_rejects(&image, PeError::BadSizeOfImage);
    }

    #[test]
    fn rejects_no_sections() {
        let mut image = base_image();
        patch_u16(&mut image, COFF + 2, 0);
        assert_rejects(&image, PeError::NoSections);
    }

    #[test]
    fn rejects_too_many_sections() {
        let mut image = base_image();
        patch_u16(&mut image, COFF + 2, MAX_SECTIONS as u16 + 1);
        assert_rejects(&image, PeError::TooManySections);
    }

    #[test]
    fn rejects_section_table_one_byte_past_eof() {
        let mut image = base_image();
        let table_end = TABLE + 4 * SECTION_HEADER_SIZE;
        image.truncate(table_end - 1);
        assert_rejects(&image, PeError::SectionTableOutOfFile);
    }

    #[test]
    fn accepts_section_table_exactly_at_eof() {
        let mut image = base_image();
        patch_dir(&mut image, IMAGE_DIRECTORY_ENTRY_BASERELOC as usize, 0, 0);
        for index in 0..4 {
            patch_section(
                &mut image,
                index,
                0x1000 + index as u32 * 0x1000,
                0x200,
                0,
                0,
            );
        }
        image.truncate(TABLE + 4 * SECTION_HEADER_SIZE);
        let pe = parse(&image).unwrap();
        assert_eq!(pe.sections().len(), 4);
    }

    #[test]
    fn rejects_unaligned_section_virtual_address() {
        let mut image = base_image();
        patch_section(&mut image, 1, 0x2001, 0x200, RDATA_RAW as u32, 0x200);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_overlapping_sections() {
        let mut image = base_image();
        patch_section(&mut image, 1, 0x1000, 0x200, RDATA_RAW as u32, 0x200);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_zero_memory_section() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0, 0, 0);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_unaligned_raw_offset() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0x200, DATA_RAW as u32 + 1, 0x200);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_unaligned_raw_size() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0x200, DATA_RAW as u32, 0x100);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_raw_size_with_zero_offset() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0x200, 0, 0x200);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_raw_offset_with_zero_size() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0x200, DATA_RAW as u32, 0);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn rejects_section_raw_data_past_eof() {
        let mut image = base_image();
        patch_section(&mut image, 2, 0x3000, 0x200, DATA_RAW as u32, 0x1000);
        assert_rejects(&image, PeError::SectionOutOfFile);
    }

    #[test]
    fn rejects_section_outside_size_of_image() {
        let mut image = base_image();
        patch_section(&mut image, 3, RELOC_RVA, 0x2001, RELOC_RAW as u32, 0x400);
        assert_rejects(&image, PeError::BadSectionLayout);
    }

    #[test]
    fn accepts_entry_at_section_start() {
        let image = base_image();
        let pe = parse(&image).unwrap();
        assert_eq!(pe.entry_rva(), 0x1000);
    }

    #[test]
    fn rejects_entry_in_non_executable_section() {
        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_ENTRY_OFFSET, 0x2000);
        assert_rejects(&image, PeError::EntryNotExecutable);
    }

    #[test]
    fn rejects_entry_one_past_executable_section() {
        let mut image = base_image();
        patch_u32(&mut image, OPTIONAL + OPT_ENTRY_OFFSET, 0x1200);
        assert_rejects(&image, PeError::EntryNotExecutable);
    }

    #[test]
    fn rejects_unsupported_subsystem() {
        let mut image = base_image();
        patch_u16(&mut image, OPTIONAL + OPT_SUBSYSTEM_OFFSET, 2);
        assert_rejects(&image, PeError::UnsupportedSubsystem);
    }

    #[test]
    fn rejects_imports() {
        let mut image = base_image();
        patch_dir(
            &mut image,
            IMAGE_DIRECTORY_ENTRY_IMPORT as usize,
            0x2000,
            0x40,
        );
        assert_rejects(&image, PeError::ImportsUnsupported);
    }

    #[test]
    fn rejects_delay_imports() {
        let mut image = base_image();
        patch_dir(
            &mut image,
            IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT as usize,
            0x2000,
            0x40,
        );
        assert_rejects(&image, PeError::DelayImportsUnsupported);
    }

    #[test]
    fn rejects_tls() {
        let mut image = base_image();
        patch_dir(&mut image, IMAGE_DIRECTORY_ENTRY_TLS as usize, 0x3000, 0x40);
        assert_rejects(&image, PeError::TlsUnsupported);
    }

    #[test]
    fn rejects_clr() {
        let mut image = base_image();
        patch_dir(
            &mut image,
            IMAGE_DIRECTORY_ENTRY_COM_DESCRIPTOR as usize,
            0x2000,
            0x48,
        );
        assert_rejects(&image, PeError::ClrUnsupported);
    }

    #[test]
    fn rejects_relocation_directory_outside_sections() {
        let mut image = base_image();
        patch_dir(
            &mut image,
            IMAGE_DIRECTORY_ENTRY_BASERELOC as usize,
            0x9000,
            12,
        );
        assert_rejects(&image, PeError::BadRelocationTable);
    }

    #[test]
    fn rejects_relocation_directory_past_section_raw_data() {
        let mut image = base_image();
        patch_dir(
            &mut image,
            IMAGE_DIRECTORY_ENTRY_BASERELOC as usize,
            RELOC_RVA,
            0x500,
        );
        assert_rejects(&image, PeError::BadRelocationTable);
    }

    #[test]
    fn rejects_relocation_block_past_directory() {
        let mut image = base_image();
        write_relocations(&mut image, &[(0x3000, &[10 << 12])]);
        patch_dir(
            &mut image,
            IMAGE_DIRECTORY_ENTRY_BASERELOC as usize,
            RELOC_RVA,
            4,
        );
        assert_rejects(&image, PeError::BadRelocationTable);
    }

    #[test]
    fn rejects_short_relocation_block() {
        let mut image = base_image();
        write_relocations(&mut image, &[(0x3000, &[10 << 12])]);
        patch_u32(&mut image, RELOC_RAW + 4, 6);
        assert_rejects(&image, PeError::BadRelocationTable);
    }

    #[test]
    fn rejects_odd_relocation_block_size() {
        let mut image = base_image();
        write_relocations(&mut image, &[(0x3000, &[10 << 12, 0])]);
        patch_u32(&mut image, RELOC_RAW + 4, 11);
        assert_rejects(&image, PeError::BadRelocationTable);
    }

    #[test]
    fn rejects_relocation_target_overflow() {
        let mut image = base_image();
        write_relocations(&mut image, &[(0xFFFF_F001, &[(10 << 12) | 0xFFF])]);
        assert_rejects(&image, PeError::BadRelocationTable);
    }

    #[test]
    fn rejects_non_dir64_relocation_type() {
        let mut image = base_image();
        write_relocations(&mut image, &[(0x3000, &[3 << 12])]);
        assert_rejects(&image, PeError::UnsupportedRelocationType);
    }

    #[test]
    fn rejects_too_many_relocations() {
        let mut image = base_image();
        let entries = vec![10 << 12; MAX_RELOCATIONS + 1];
        write_relocations(&mut image, &[(0x3000, &entries)]);
        assert_rejects(&image, PeError::TooManyRelocations);
    }

    #[test]
    fn accepts_exactly_max_relocations() {
        let mut image = base_image();
        let entries = vec![10 << 12; MAX_RELOCATIONS];
        write_relocations(&mut image, &[(0x3000, &entries)]);
        let pe = parse(&image).unwrap();
        assert_eq!(pe.relocations().len(), MAX_RELOCATIONS);
    }
}
