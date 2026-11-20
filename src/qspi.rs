use stm32h7xx_hal::{
    pac,
    xspi::{Qspi, QspiError, QspiWord},
};

#[derive(Debug)]
pub enum FlashError {
    OutOfBounds,
    Qspi(QspiError),
}

pub struct W25q64jv {
    qspi: Qspi<pac::QUADSPI>,
}

impl W25q64jv {
    pub fn new(qspi: Qspi<pac::QUADSPI>) -> Self {
        Self { qspi }
    }

    pub fn jedec_id(&mut self) -> Result<[u8; 3], QspiError> {
        let mut id = [0u8; 3];
        self.qspi.read_extended(
            QspiWord::U8(0x9f),
            QspiWord::None,
            QspiWord::None,
            0,
            &mut id,
        )?;

        Ok(id)
    }

    pub fn read_at(&mut self, offize: u32, buffer: &mut [u8]) -> Result<(), FlashError> {
        const FLASH_SIZE: u32 = 8 * 1024 * 1024;
        const MAX_READ_SIZE: usize = 32;
        let len = u32::try_from(buffer.len()).map_err(|_| FlashError::OutOfBounds)?;
        let end = offize.checked_add(len).ok_or(FlashError::OutOfBounds)?;

        if end > FLASH_SIZE {
            return Err(FlashError::OutOfBounds);
        }

        let mut addr = offize;
        for chunk in buffer.chunks_mut(MAX_READ_SIZE) {
            self.qspi
                .read_extended(
                    QspiWord::U8(0x03),
                    QspiWord::U24(addr),
                    QspiWord::None,
                    0,
                    chunk,
                )
                .map_err(FlashError::Qspi)?;

            addr += chunk.len() as u32;
        }

        Ok(())
    }
}
