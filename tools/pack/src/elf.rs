const ELF_HEADER_SIZE: usize = 52;
const PROGRAM_HEADER_SIZE: usize = 32;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;

const APP_LOAD_START: u32 = 0x2406_0000;
// The last 4 KiB of user SRAM is reserved for the stack.
const APP_LOAD_END: u32 = 0x2407_F000;
const MAX_SEGMENTS: usize = 16;

pub(super) struct Segment {
    pub dst_addr: u32,
    pub payload: Vec<u8>,
    pub mem_len: u32,
    pub flags: u32,
}

pub(super) fn parse(elf: &[u8]) -> Result<(u32, Vec<Segment>), String> {
    let header = elf
        .first_chunk::<ELF_HEADER_SIZE>()
        .ok_or("truncated ELF header")?;
    let file_type = read_u16(header, 16);
    let machine = read_u16(header, 18);
    let version = read_u32(header, 20);
    // ELFCLASS32, ELFDATA2LSB, EV_CURRENT, ET_EXEC and EM_ARM.
    if &header[..7] != b"\x7fELF\x01\x01\x01" || (file_type, machine, version) != (2, 40, 1) {
        return Err("expected a little-endian ARM ELF32 executable".into());
    }

    let entry = read_u32(header, 24);
    let table_start = read_u32(header, 28) as usize;
    let header_size = usize::from(read_u16(header, 40));
    let entry_size = usize::from(read_u16(header, 42));
    let entry_count = usize::from(read_u16(header, 44));
    if header_size != ELF_HEADER_SIZE
        || table_start < ELF_HEADER_SIZE
        || entry_size != PROGRAM_HEADER_SIZE
    {
        return Err("invalid ELF program header table".into());
    }
    let table = elf
        .get(table_start..)
        .and_then(|tail| tail.get(..entry_size * entry_count))
        .ok_or("invalid ELF program header table")?;

    let mut segments: Vec<Segment> = Vec::new();
    let (records, _) = table.as_chunks::<PROGRAM_HEADER_SIZE>();
    for (index, record) in records.iter().enumerate() {
        if read_u32(record, 0) != PT_LOAD {
            continue;
        }
        let file_offset = read_u32(record, 4) as usize;
        let dst_addr = read_u32(record, 8);
        let file_len = read_u32(record, 16);
        let mem_len = read_u32(record, 20);
        let flags = read_u32(record, 24);

        if file_len > mem_len {
            return Err(format!("PT_LOAD {index}: file size exceeds memory size"));
        }
        // A zero-file-size segment has no source bytes to read.
        let payload = if file_len == 0 {
            &[]
        } else {
            elf.get(file_offset..)
                .and_then(|tail| tail.get(..file_len as usize))
                .ok_or_else(|| format!("PT_LOAD {index}: file range outside ELF"))?
        };
        if mem_len == 0 {
            continue;
        }
        let end = dst_addr
            .checked_add(mem_len)
            .filter(|&end| dst_addr >= APP_LOAD_START && end <= APP_LOAD_END)
            .ok_or_else(|| format!("PT_LOAD {index}: address range outside application SRAM"))?;
        if segments
            .iter()
            .any(|segment| dst_addr < segment.dst_addr + segment.mem_len && segment.dst_addr < end)
        {
            return Err(format!("PT_LOAD {index}: overlaps another load segment"));
        }
        if segments.len() == MAX_SEGMENTS {
            return Err(format!(
                "at most {MAX_SEGMENTS} load segments per application"
            ));
        }
        segments.push(Segment {
            dst_addr,
            payload: payload.to_vec(),
            mem_len,
            flags,
        });
    }

    // Keep the Thumb bit in the image, but check the actual instruction address.
    let entry_addr = entry & !1;
    if entry & 1 == 0
        || !segments.iter().any(|segment| {
            segment.flags & PF_X != 0
                && (segment.dst_addr..segment.dst_addr + segment.payload.len() as u32)
                    .contains(&entry_addr)
        })
    {
        return Err("entry must be a Thumb address in initialized executable data".into());
    }
    Ok((entry, segments))
}

// Callers provide a complete ELF header or program header and fixed field offsets.
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
