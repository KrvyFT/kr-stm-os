use crate::elf::Segment;

const HEADER_SIZE: usize = 24;
const APP_RECORD_SIZE: usize = 28;
const SEGMENT_RECORD_SIZE: usize = 20;
const CRC_OFFSET: usize = 20;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

pub(super) const MAX_APPS: usize = 16;

pub(super) struct App {
    pub name: [u8; 16],
    pub entry: u32,
    pub segments: Vec<Segment>,
}

pub(super) fn encode(apps: &[App]) -> Result<Vec<u8>, String> {
    let segment_count = apps.iter().map(|app| app.segments.len()).sum::<usize>();
    let payload_len = apps
        .iter()
        .flat_map(|app| &app.segments)
        .map(|segment| segment.payload.len())
        .sum::<usize>();

    // Lay out both tables before the payload so the records can store absolute image offsets.
    let mut segment_offset = HEADER_SIZE + apps.len() * APP_RECORD_SIZE;
    let mut payload_offset = segment_offset + segment_count * SEGMENT_RECORD_SIZE;
    let total_len = payload_offset + payload_len;
    if total_len > MAX_IMAGE_BYTES {
        return Err("image exceeds the 8 MiB QSPI flash capacity".into());
    }

    let mut image = vec![0; total_len];
    image[..4].copy_from_slice(b"KAPP");
    write_u32s(
        &mut image[4..HEADER_SIZE],
        [
            1,
            total_len as u32,
            apps.len() as u32,
            HEADER_SIZE as u32,
            0,
        ],
    );

    for (index, app) in apps.iter().enumerate() {
        let app_offset = HEADER_SIZE + index * APP_RECORD_SIZE;
        let record = &mut image[app_offset..app_offset + APP_RECORD_SIZE];
        record[..16].copy_from_slice(&app.name);
        write_u32s(
            &mut record[16..],
            [app.entry, app.segments.len() as u32, segment_offset as u32],
        );

        for segment in &app.segments {
            write_u32s(
                &mut image[segment_offset..segment_offset + SEGMENT_RECORD_SIZE],
                [
                    segment.dst_addr,
                    payload_offset as u32,
                    segment.payload.len() as u32,
                    segment.mem_len,
                    segment.flags,
                ],
            );
            segment_offset += SEGMENT_RECORD_SIZE;

            // Only initialized bytes occupy flash. The kernel zeroes the rest in SRAM.
            let payload_end = payload_offset + segment.payload.len();
            image[payload_offset..payload_end].copy_from_slice(&segment.payload);
            payload_offset = payload_end;
        }
    }

    // The CRC covers the entire image with the checksum field still zero.
    let checksum = crc32(&image);
    image[CRC_OFFSET..HEADER_SIZE].copy_from_slice(&checksum.to_le_bytes());
    Ok(image)
}

fn write_u32s<const N: usize>(output: &mut [u8], values: [u32; N]) {
    debug_assert_eq!(output.len(), N * size_of::<u32>());
    let (words, _) = output.as_chunks_mut::<{ size_of::<u32>() }>();
    for (bytes, value) in words.iter_mut().zip(values) {
        *bytes = value.to_le_bytes();
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    // Reflected IEEE CRC32, matching zlib.crc32 in the Python packer.
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xedb8_8320
            };
        }
    }
    !crc
}
