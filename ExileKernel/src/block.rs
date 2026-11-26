//! Checked block-I/O request contract; real controller drivers implement BlockDevice.
#![no_std]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    ZeroCount,
    OutOfRange,
    Overflow,
    BufferSize,
    ReadOnly,
    Device,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRequest {
    pub lba: u64,
    pub blocks: u32,
}
impl BlockRequest {
    pub fn validate(self, capacity: u64) -> Result<u64, BlockError> {
        if self.blocks == 0 {
            return Err(BlockError::ZeroCount);
        }
        let e = self
            .lba
            .checked_add(self.blocks as u64)
            .ok_or(BlockError::Overflow)?;
        if e > capacity {
            return Err(BlockError::OutOfRange);
        }
        Ok(e)
    }
}
pub trait BlockDevice {
    fn block_size(&self) -> u32;
    fn block_count(&self) -> u64;
    fn read_blocks(&mut self, r: BlockRequest, out: &mut [u8]) -> Result<(), BlockError>;
    fn write_blocks(&mut self, r: BlockRequest, input: &[u8]) -> Result<(), BlockError>;
}
pub fn read<D: BlockDevice>(d: &mut D, r: BlockRequest, out: &mut [u8]) -> Result<(), BlockError> {
    r.validate(d.block_count())?;
    let n = (r.blocks as usize)
        .checked_mul(d.block_size() as usize)
        .ok_or(BlockError::Overflow)?;
    if d.block_size() == 0 || out.len() != n {
        return Err(BlockError::BufferSize);
    }
    d.read_blocks(r, out)
}
pub fn write<D: BlockDevice>(d: &mut D, r: BlockRequest, input: &[u8]) -> Result<(), BlockError> {
    r.validate(d.block_count())?;
    let n = (r.blocks as usize)
        .checked_mul(d.block_size() as usize)
        .ok_or(BlockError::Overflow)?;
    if d.block_size() == 0 || input.len() != n {
        return Err(BlockError::BufferSize);
    }
    d.write_blocks(r, input)
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Mem {
        b: [u8; 32],
        ro: bool,
    }
    impl BlockDevice for Mem {
        fn block_size(&self) -> u32 {
            8
        }
        fn block_count(&self) -> u64 {
            4
        }
        fn read_blocks(&mut self, r: BlockRequest, o: &mut [u8]) -> Result<(), BlockError> {
            let s = r.lba as usize * 8;
            o.copy_from_slice(&self.b[s..s + o.len()]);
            Ok(())
        }
        fn write_blocks(&mut self, r: BlockRequest, i: &[u8]) -> Result<(), BlockError> {
            if self.ro {
                return Err(BlockError::ReadOnly);
            }
            let s = r.lba as usize * 8;
            self.b[s..s + i.len()].copy_from_slice(i);
            Ok(())
        }
    }
    #[test]
    fn io() {
        let mut d = Mem {
            b: [0; 32],
            ro: false,
        };
        write(&mut d, BlockRequest { lba: 1, blocks: 1 }, b"12345678").unwrap();
        let mut b = [0; 8];
        read(&mut d, BlockRequest { lba: 1, blocks: 1 }, &mut b).unwrap();
        assert_eq!(&b, b"12345678");
    }
    #[test]
    fn bounds_and_sizes() {
        let mut d = Mem {
            b: [0; 32],
            ro: true,
        };
        assert_eq!(
            BlockRequest {
                lba: u64::MAX,
                blocks: 2
            }
            .validate(4),
            Err(BlockError::Overflow)
        );
        assert_eq!(
            read(&mut d, BlockRequest { lba: 4, blocks: 1 }, &mut [0; 8]),
            Err(BlockError::OutOfRange)
        );
        assert_eq!(
            read(&mut d, BlockRequest { lba: 0, blocks: 1 }, &mut [0; 7]),
            Err(BlockError::BufferSize)
        );
        assert_eq!(
            write(&mut d, BlockRequest { lba: 0, blocks: 1 }, &[0; 8]),
            Err(BlockError::ReadOnly)
        );
    }
}
