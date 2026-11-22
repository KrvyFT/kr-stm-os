use heapless::Vec;

use crate::{
    batch::{APP_END, APP_START},
    qspi::{FlashError, W25q64jv},
};

const HEADER_SIZE: u32 = 24;
const APP_SIZE: u32 = 28;
const SEGMENT_SIZE: u32 = 20;
const MAX_APPS: u32 = 16;
const MAX_SEGMENTS: usize = 16;
const FLASH_SIZE: u32 = 8 * 1024 * 1024;
const PF_X: u32 = 1;

#[derive(Debug)]
pub(crate) enum ImageError {
    Flash(FlashError),
    NoImage,
    InvalidHeader,
    InvalidApp(u32),
    InvalidSegment(u32, u32),
    InvalidEntry(u32),
    BadCrc,
}

struct ImageHeader {
    total_len: u32,
    app_count: u32,
    app_table_offset: u32,
    image_crc32: u32,
}

struct AppRecord {
    name: [u8; 16],
    entry: u32,
    segment_count: u32,
    segment_table_offset: u32,
}

pub struct SegmentRecord {
    pub dst_addr: u32,
    pub payload_offset: u32,
    pub file_len: u32,
    pub mem_len: u32,
    pub flags: u32,
}

pub struct ValidatedApp {
    name: [u8; 16],
    entry: u32,
    segments: heapless::Vec<SegmentRecord, MAX_SEGMENTS>,
}

impl ValidatedApp {
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        str::from_utf8(&self.name[..end]).expect("validated ASCII name")
    }

    pub fn entry(&self) -> u32 {
        self.entry
    }

    pub fn segments(&self) -> &[SegmentRecord] {
        self.segments.as_slice()
    }
}

pub struct Image<'a> {
    flash: &'a mut W25q64jv,
    header: ImageHeader,
    payload_start: u32,
}

fn le_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

impl Image<'_> {
    pub fn open(flash: &mut W25q64jv) -> Result<Image<'_>, ImageError> {
        let mut raw = [0u8; HEADER_SIZE as usize];
        flash
            .read_at(0, &mut raw)
            .map_err(|e| ImageError::Flash(e))?;

        if raw[..4] == [0xff; 4] {
            return Err(ImageError::NoImage);
        }
        if &raw[..4] != b"KAPP" || le_u32(&raw, 4) != 1 {
            return Err(ImageError::InvalidHeader);
        }

        let header = ImageHeader {
            total_len: le_u32(&raw, 8),
            app_count: le_u32(&raw, 12),
            app_table_offset: le_u32(&raw, 16),
            image_crc32: le_u32(&raw, 20),
        };

        if header.app_count > MAX_APPS
            || header.app_table_offset != HEADER_SIZE
            || !(HEADER_SIZE..=FLASH_SIZE).contains(&header.total_len)
        {
            return Err(ImageError::InvalidHeader);
        }

        let mut segment_cursor = header.app_table_offset + header.app_count * APP_SIZE;
        if segment_cursor >= header.total_len {
            return Err(ImageError::InvalidHeader);
        }

        let mut image = Image {
            flash,
            header,
            payload_start: 0,
        };

        for index in 0..image.header.app_count {
            let app = image.read_app(index)?;
            if !(1..=MAX_SEGMENTS as u32).contains(&app.segment_count)
                || app.segment_table_offset != segment_cursor
            {
                return Err(ImageError::InvalidApp(index));
            }
            segment_cursor += app.segment_count * SEGMENT_SIZE;
            if segment_cursor > image.header.total_len {
                return Err(ImageError::InvalidApp(index));
            }
        }
        image.payload_start = segment_cursor;

        Ok(image)
    }

    pub fn app_count(&self) -> u32 {
        self.header.app_count
    }

    fn read_app(&mut self, index: u32) -> Result<AppRecord, ImageError> {
        if index >= self.app_count() {
            return Err(ImageError::InvalidApp(index));
        }

        let mut raw = [0; APP_SIZE as usize];
        let offset = self.header.app_table_offset + index * APP_SIZE;
        self.flash
            .read_at(offset, &mut raw)
            .map_err(ImageError::Flash)?;

        Ok(AppRecord {
            name: raw[..16].try_into().unwrap(),
            entry: le_u32(&raw, 16),
            segment_count: le_u32(&raw, 20),
            segment_table_offset: le_u32(&raw, 24),
        })
    }

    pub fn validate_app(&mut self, index: u32) -> Result<ValidatedApp, ImageError> {
        let app = self.read_app(index)?;
        let name_end = app.name.iter().position(|&b| b == 0).unwrap_or(16);
        if name_end == 0
            || !app.name[..name_end]
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            || app.name[name_end..].iter().any(|&b| b != 0)
        {
            return Err(ImageError::InvalidApp(index));
        }

        let mut segments = Vec::<SegmentRecord, MAX_SEGMENTS>::new();
        for n in 0..app.segment_count {
            let mut raw = [0; SEGMENT_SIZE as usize];
            let offset = app.segment_table_offset + n * SEGMENT_SIZE;
            self.flash
                .read_at(offset, &mut raw)
                .map_err(ImageError::Flash)?;

            let segment = SegmentRecord {
                dst_addr: le_u32(&raw, 0),
                payload_offset: le_u32(&raw, 4),
                file_len: le_u32(&raw, 8),
                mem_len: le_u32(&raw, 12),
                flags: le_u32(&raw, 16),
            };

            let invalid = || ImageError::InvalidSegment(index, n);
            let dst_end = segment
                .dst_addr
                .checked_add(segment.mem_len)
                .ok_or_else(invalid)?;
            let src_end = segment
                .payload_offset
                .checked_add(segment.file_len)
                .ok_or_else(invalid)?;

            if segment.mem_len == 0
                || segment.file_len > segment.mem_len
                || segment.dst_addr < APP_START
                || dst_end > APP_END
                || segment.payload_offset < self.payload_start
                || src_end > self.header.total_len
                || {
                    segments.iter().any(|prev| {
                        prev.dst_addr < dst_end && segment.dst_addr < prev.dst_addr + prev.mem_len
                    })
                }
            {
                return Err(invalid());
            }

            segments.push(segment).map_err(|_| invalid())?;
        }

        let entry_addr = app.entry & !1;

        if app.entry & 1 == 0
            || !segments.iter().any(|segment| {
                segment.flags & PF_X != 0
                    && segment.dst_addr <= entry_addr
                    && entry_addr < segment.dst_addr + segment.file_len
            })
        {
            return Err(ImageError::InvalidEntry(index));
        }

        Ok(ValidatedApp {
            name: app.name,
            entry: app.entry,
            segments,
        })
    }

    pub fn read_payload(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), ImageError> {
        self.flash.read_at(offset, buf).map_err(ImageError::Flash)
    }

    fn verify_crc(&mut self) -> Result<(), ImageError> {
        let mut crc = !0u32;
        let mut offset = 0;
        let mut buf = [0u8; 128];

        while offset < self.header.total_len {
            let len = (self.header.total_len - offset).min(buf.len() as u32) as usize;
            self.flash
                .read_at(offset, &mut buf[..len])
                .map_err(ImageError::Flash)?;

            for (i, &byte) in buf[..len].iter().enumerate() {
                // 打包器计算 CRC 时，这四字节仍为零。
                let byte = if (20..24).contains(&(offset + i as u32)) {
                    0
                } else {
                    byte
                };
                crc ^= u32::from(byte);
                for _ in 0..8 {
                    let low_bit = crc & 1;
                    crc >>= 1;
                    if low_bit != 0 {
                        crc ^= 0xedb8_8320;
                    }
                }
            }
            offset += len as u32;
        }

        if !crc != self.header.image_crc32 {
            return Err(ImageError::BadCrc);
        }
        Ok(())
    }
}
