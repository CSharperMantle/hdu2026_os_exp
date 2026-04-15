use crate::BlockId;

/// Abstraction of a block I/O.
pub trait BlockDevice {
    fn block_size(&self) -> usize;
    fn block_count(&self) -> usize;
    fn read_block(&self, index: BlockId) -> &[u8];
    fn write_block(&mut self, index: BlockId, data: &[u8]);
    fn zero_block(&mut self, index: BlockId);
}

/// RAM-backed block device.
#[derive(Debug)]
pub struct MemoryBlockDevice {
    block_size: usize,
    blocks: Vec<Vec<u8>>,
}

impl MemoryBlockDevice {
    pub fn new(block_size: usize, block_count: usize) -> Self {
        Self {
            block_size,
            blocks: vec![vec![0; block_size]; block_count],
        }
    }
}

impl BlockDevice for MemoryBlockDevice {
    fn block_size(&self) -> usize {
        self.block_size
    }

    fn block_count(&self) -> usize {
        self.blocks.len()
    }

    fn read_block(&self, index: BlockId) -> &[u8] {
        &self.blocks[usize::from(index.0)]
    }

    fn write_block(&mut self, index: BlockId, data: &[u8]) {
        let block = &mut self.blocks[usize::from(index.0)];
        block.copy_from_slice(data);
    }

    fn zero_block(&mut self, index: BlockId) {
        self.blocks[usize::from(index.0)].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_block_device_reads_writes_and_zeros_blocks() {
        let mut dev = MemoryBlockDevice::new(8, 4);
        assert_eq!(dev.block_size(), 8);
        assert_eq!(dev.block_count(), 4);

        dev.write_block(BlockId(2), &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(dev.read_block(BlockId(2)), &[1, 2, 3, 4, 5, 6, 7, 8]);

        dev.zero_block(BlockId(2));
        assert_eq!(dev.read_block(BlockId(2)), &[0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
