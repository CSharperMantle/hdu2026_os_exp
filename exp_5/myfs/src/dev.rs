//! Device abstractions.

use crate::BlockId;
use crate::FsError;
use std::fs::File;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;

/// Physical device block identifier.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PhysicalBlockId(pub usize);

/// Physical block device I/O.
///
/// This models the underlying storage granularity. Callers provide buffers and
/// all operations may fail.
pub trait PhysicalBlockDevice {
    fn physical_block_size(&self) -> usize;
    fn physical_block_count(&self) -> usize;
    fn read_physical_block(
        &mut self,
        index: PhysicalBlockId,
        dst: &mut [u8],
    ) -> Result<(), FsError>;
    fn write_physical_block(&mut self, index: PhysicalBlockId, src: &[u8]) -> Result<(), FsError>;

    fn zero_physical_block(&mut self, index: PhysicalBlockId) -> Result<(), FsError> {
        let zeros = vec![0; self.physical_block_size()];
        self.write_physical_block(index, &zeros)
    }
}

/// Fallible logical-block I/O layered over physical blocks.
///
/// This is directly consumed by [`crate::MyFileSystem`].
pub trait BufferedBlockDevice {
    fn block_size(&self) -> usize;
    fn block_count(&self) -> usize;
    fn read_block_into(&mut self, index: BlockId, dst: &mut [u8]) -> Result<(), FsError>;
    fn write_block_from(&mut self, index: BlockId, src: &[u8]) -> Result<(), FsError>;

    fn zero_block(&mut self, index: BlockId) -> Result<(), FsError> {
        let zeros = vec![0; self.block_size()];
        self.write_block_from(index, &zeros)
    }
}

/// Adapter exposing filesystem logical blocks on top of a physical device.
///
/// FIXME: Current restriction: one logical block must be an integer multiple of one
/// physical block, and cannot be smaller.
#[derive(Debug)]
pub struct LogicalBlockDevice<P> {
    inner: P,
    logical_block_size: usize,
    logical_block_count: usize,
    physical_blocks_per_logical: usize,
}

impl<P: PhysicalBlockDevice> LogicalBlockDevice<P> {
    pub fn new(inner: P, logical_block_size: usize) -> Result<Self, FsError> {
        let physical_block_size = inner.physical_block_size();
        if logical_block_size < physical_block_size {
            return Err(FsError::InvalidConfig(format!(
                "logical block size {} smaller than physical block size {}",
                logical_block_size, physical_block_size
            )));
        }
        if !logical_block_size.is_multiple_of(physical_block_size) {
            return Err(FsError::InvalidConfig(format!(
                "logical block size {} not multiple of physical block size {}",
                logical_block_size, physical_block_size
            )));
        }

        let physical_blocks_per_logical = logical_block_size / physical_block_size;
        if !inner
            .physical_block_count()
            .is_multiple_of(physical_blocks_per_logical)
        {
            return Err(FsError::InvalidConfig(format!(
                "physical block count {} not divisible by physical blocks per logical block {}",
                inner.physical_block_count(),
                physical_blocks_per_logical
            )));
        }

        let logical_block_count = inner.physical_block_count() / physical_blocks_per_logical;
        Ok(Self {
            inner,
            logical_block_size,
            logical_block_count,
            physical_blocks_per_logical,
        })
    }

    pub fn into_inner(self) -> P {
        self.inner
    }

    fn physical_start_of(&self, index: BlockId) -> Result<usize, FsError> {
        let logical_index = usize::from(u16::from(index));
        if logical_index >= self.logical_block_count {
            return Err(FsError::InvalidConfig(format!(
                "logical block {} outside device range {}",
                logical_index, self.logical_block_count
            )));
        }
        Ok(logical_index * self.physical_blocks_per_logical)
    }
}

impl<P: PhysicalBlockDevice> BufferedBlockDevice for LogicalBlockDevice<P> {
    fn block_size(&self) -> usize {
        self.logical_block_size
    }

    fn block_count(&self) -> usize {
        self.logical_block_count
    }

    fn read_block_into(&mut self, index: BlockId, dst: &mut [u8]) -> Result<(), FsError> {
        if dst.len() != self.logical_block_size {
            return Err(FsError::InvalidConfig(format!(
                "buffer size {} does not match logical block size {}",
                dst.len(),
                self.logical_block_size
            )));
        }

        let physical_block_size = self.inner.physical_block_size();
        let physical_start = self.physical_start_of(index)?;
        for offset in 0..self.physical_blocks_per_logical {
            let start = offset * physical_block_size;
            let end = start + physical_block_size;
            self.inner.read_physical_block(
                PhysicalBlockId(physical_start + offset),
                &mut dst[start..end],
            )?;
        }
        Ok(())
    }

    fn write_block_from(&mut self, index: BlockId, src: &[u8]) -> Result<(), FsError> {
        if src.len() != self.logical_block_size {
            return Err(FsError::InvalidConfig(format!(
                "buffer size {} does not match logical block size {}",
                src.len(),
                self.logical_block_size
            )));
        }

        let physical_block_size = self.inner.physical_block_size();
        let physical_start = self.physical_start_of(index)?;
        for offset in 0..self.physical_blocks_per_logical {
            let start = offset * physical_block_size;
            let end = start + physical_block_size;
            self.inner
                .write_physical_block(PhysicalBlockId(physical_start + offset), &src[start..end])?;
        }
        Ok(())
    }
}

/// RAM-backed physical device.
#[derive(Debug)]
pub struct MemoryBackend {
    block_size: usize,
    blocks: Vec<Vec<u8>>,
}

impl MemoryBackend {
    pub fn new(block_size: usize, block_count: usize) -> Self {
        Self {
            block_size,
            blocks: vec![vec![0; block_size]; block_count],
        }
    }
}

impl PhysicalBlockDevice for MemoryBackend {
    fn physical_block_size(&self) -> usize {
        self.block_size
    }

    fn physical_block_count(&self) -> usize {
        self.blocks.len()
    }

    fn read_physical_block(
        &mut self,
        index: PhysicalBlockId,
        dst: &mut [u8],
    ) -> Result<(), FsError> {
        if dst.len() != self.block_size {
            return Err(FsError::InvalidConfig(format!(
                "buffer size {} does not match physical block size {}",
                dst.len(),
                self.block_size
            )));
        }
        let block = self.blocks.get(index.0).ok_or_else(|| {
            FsError::InvalidConfig(format!(
                "physical block {} outside device range {}",
                index.0,
                self.blocks.len()
            ))
        })?;
        dst.copy_from_slice(block);
        Ok(())
    }

    fn write_physical_block(&mut self, index: PhysicalBlockId, src: &[u8]) -> Result<(), FsError> {
        if src.len() != self.block_size {
            return Err(FsError::InvalidConfig(format!(
                "buffer size {} does not match physical block size {}",
                src.len(),
                self.block_size
            )));
        }
        let block_count = self.blocks.len();
        let block = self.blocks.get_mut(index.0).ok_or_else(|| {
            FsError::InvalidConfig(format!(
                "physical block {} outside device range {}",
                index.0, block_count
            ))
        })?;
        block.copy_from_slice(src);
        Ok(())
    }
}

/// File-backed physical block device.
#[derive(Debug)]
pub struct FileBackend {
    file: File,
    block_size: usize,
    block_count: usize,
}

impl FileBackend {
    pub fn from_file(file: File, block_size: usize) -> Result<Self, FsError> {
        if block_size == 0 {
            return Err(FsError::InvalidConfig(
                "block size must be at least 1".to_string(),
            ));
        }
        let file_len = file
            .metadata()
            .map_err(|err| FsError::CorruptFs(format!("file metadata failed: {err}")))?
            .len();
        let block_size_u64 = u64::try_from(block_size).unwrap();
        if file_len % block_size_u64 != 0 {
            return Err(FsError::InvalidConfig(format!(
                "file size {} not divisible by block size {}",
                file_len, block_size
            )));
        }
        Ok(Self {
            file,
            block_size,
            block_count: usize::try_from(file_len / block_size_u64).unwrap(),
        })
    }

    pub fn create(file: File, block_size: usize, block_count: usize) -> Result<Self, FsError> {
        if block_size == 0 {
            return Err(FsError::InvalidConfig(
                "block size must be at least 1".to_string(),
            ));
        }
        let len = u64::try_from(block_size.checked_mul(block_count).ok_or_else(|| {
            FsError::InvalidConfig("file-backed device size overflow".to_string())
        })?)
        .unwrap();
        file.set_len(len)
            .map_err(|err| FsError::CorruptFs(format!("set_len failed: {err}")))?;
        Self::from_file(file, block_size)
    }

    pub fn sync(&mut self) -> Result<(), FsError> {
        self.file
            .sync_all()
            .map_err(|err| FsError::CorruptFs(format!("sync_all failed: {err}")))
    }

    fn seek_to_block(&mut self, index: PhysicalBlockId) -> Result<(), FsError> {
        if index.0 >= self.block_count {
            return Err(FsError::InvalidConfig(format!(
                "physical block {} outside device range {}",
                index.0, self.block_count
            )));
        }
        let offset = u64::try_from(index.0.checked_mul(self.block_size).ok_or_else(|| {
            FsError::InvalidConfig("file-backed block offset overflow".to_string())
        })?)
        .unwrap();
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|err| FsError::CorruptFs(format!("seek failed: {err}")))?;
        Ok(())
    }
}

impl PhysicalBlockDevice for FileBackend {
    fn physical_block_size(&self) -> usize {
        self.block_size
    }

    fn physical_block_count(&self) -> usize {
        self.block_count
    }

    fn read_physical_block(
        &mut self,
        index: PhysicalBlockId,
        dst: &mut [u8],
    ) -> Result<(), FsError> {
        if dst.len() != self.block_size {
            return Err(FsError::InvalidConfig(format!(
                "buffer size {} does not match physical block size {}",
                dst.len(),
                self.block_size
            )));
        }
        self.seek_to_block(index)?;
        self.file
            .read_exact(dst)
            .map_err(|err| FsError::CorruptFs(format!("read_exact failed: {err}")))?;
        Ok(())
    }

    fn write_physical_block(&mut self, index: PhysicalBlockId, src: &[u8]) -> Result<(), FsError> {
        if src.len() != self.block_size {
            return Err(FsError::InvalidConfig(format!(
                "buffer size {} does not match physical block size {}",
                src.len(),
                self.block_size
            )));
        }
        self.seek_to_block(index)?;
        self.file
            .write_all(src)
            .map_err(|err| FsError::CorruptFs(format!("write_all failed: {err}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use tempfile::tempdir;

    #[test]
    fn memory_backend_supports_physical_buffer_io() {
        let mut dev = MemoryBackend::new(4, 3);
        dev.write_physical_block(PhysicalBlockId(1), &[9, 8, 7, 6])
            .unwrap();

        let mut out = [0u8; 4];
        dev.read_physical_block(PhysicalBlockId(1), &mut out)
            .unwrap();
        assert_eq!(out, [9, 8, 7, 6]);

        dev.zero_physical_block(PhysicalBlockId(1)).unwrap();
        dev.read_physical_block(PhysicalBlockId(1), &mut out)
            .unwrap();
        assert_eq!(out, [0, 0, 0, 0]);
    }

    #[test]
    fn logical_block_device_reads_and_writes_multiple_physical_blocks() {
        let inner = MemoryBackend::new(4, 8);
        let mut dev = LogicalBlockDevice::new(inner, 8).unwrap();
        assert_eq!(dev.block_size(), 8);
        assert_eq!(dev.block_count(), 4);

        dev.write_block_from(BlockId(1), &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();

        let mut out = [0u8; 8];
        dev.read_block_into(BlockId(1), &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn logical_block_device_over_memory_supports_equal_size_blocks() {
        let inner = MemoryBackend::new(8, 4);
        let mut dev = LogicalBlockDevice::new(inner, 8).unwrap();
        assert_eq!(dev.block_size(), 8);
        assert_eq!(dev.block_count(), 4);

        dev.write_block_from(BlockId(2), &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();

        let mut out = [0u8; 8];
        dev.read_block_into(BlockId(2), &mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 7, 8]);

        dev.zero_block(BlockId(2)).unwrap();
        dev.read_block_into(BlockId(2), &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn logical_block_device_rejects_invalid_block_size_relation() {
        let inner = MemoryBackend::new(8, 8);
        assert!(LogicalBlockDevice::new(inner, 4).is_err());

        let inner = MemoryBackend::new(6, 8);
        assert!(LogicalBlockDevice::new(inner, 8).is_err());
    }

    #[test]
    fn logical_block_device_rejects_partial_tail() {
        let inner = MemoryBackend::new(4, 7);
        assert!(LogicalBlockDevice::new(inner, 8).is_err());
    }

    #[test]
    fn file_backend_reads_writes_and_zeros_blocks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("file-block-device.img");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let mut dev = FileBackend::create(file, 4, 3).unwrap();

        dev.write_physical_block(PhysicalBlockId(1), &[1, 2, 3, 4])
            .unwrap();

        let mut out = [0u8; 4];
        dev.read_physical_block(PhysicalBlockId(1), &mut out)
            .unwrap();
        assert_eq!(out, [1, 2, 3, 4]);

        dev.zero_physical_block(PhysicalBlockId(1)).unwrap();
        dev.read_physical_block(PhysicalBlockId(1), &mut out)
            .unwrap();
        assert_eq!(out, [0, 0, 0, 0]);

        dev.sync().unwrap();
    }

    #[test]
    fn logical_block_device_works_on_file_backend() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("logical-file-block-device.img");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let inner = FileBackend::create(file, 4, 8).unwrap();
        let mut dev = LogicalBlockDevice::new(inner, 8).unwrap();

        dev.write_block_from(BlockId(2), &[9, 8, 7, 6, 5, 4, 3, 2])
            .unwrap();

        let mut out = [0u8; 8];
        dev.read_block_into(BlockId(2), &mut out).unwrap();
        assert_eq!(out, [9, 8, 7, 6, 5, 4, 3, 2]);
    }
}
