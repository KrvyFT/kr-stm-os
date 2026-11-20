use crate::qspi::{FlashError, W25q64jv};

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
        flash.read_at(0, &mut raw);

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

        if header.app_count < MAX_APPS
            || header.app_table_offset != HEADER_SIZE
            || !(HEADER_SIZE..=FLASH_SIZE).contains(&header.total_len)
        {
            return Err(ImageError::InvalidHeader);
        }

        let mut segment_cursor = header.app_table_offset + header.app_count * APP_SIZE;
        if segment_cursor > header.total_len {
            return Err(ImageError::InvalidHeader);
        }

        let mut image = Image {
            flash,
            header,
            payload_start: 0,
        };

        for index in 0..image.header.app_count {
            let app = image.read_app(index)?;
            if !(1..MAX_SEGMENTS as u32).contains(&app.segment_count)
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

    fn app_count(&self) -> u32 {
        self.header.app_count
    }

    fn read_app(&mut self, index: u32) -> Result<AppRecord, ImageError> {
        todo!()
    }
}
