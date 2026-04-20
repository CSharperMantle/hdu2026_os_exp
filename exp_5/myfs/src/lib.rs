mod dev;
mod fs;
mod name;

pub use dev::*;
pub use fs::*;
pub use name::*;

use std::error;
use std::fmt;

pub const MAX_OPEN_FILES: usize = 10;

/// Location of a directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DirEntryLoc {
    pub dir_start: ClusterId,
    pub entry_index: u32,
}

impl fmt::Display for DirEntryLoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.dir_start, self.entry_index)
    }
}

/// Error type for [`MyFileSystem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    InvalidConfig(String),
    InvalidName(String),
    InvalidPath(String),
    NotFound(String),
    NotFoundAt(DirEntryLoc),
    NotADirectory(String),
    IsADirectory(String),
    DirectoryNotEmpty(String),
    NoSpace,
    TooManyOpenFiles,
    AlreadyOpen(DirEntryLoc),
    FileOpen(DirEntryLoc),
    InvalidHandle(FileHandle),
    SeekOutOfBounds(usize),
    CorruptFs(String),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsError::InvalidConfig(msg) => write!(f, "invalid config: {msg}"),
            FsError::InvalidName(name) => write!(f, "invalid 8.3 name: {name}"),
            FsError::InvalidPath(path) => write!(f, "invalid path: {path}"),
            FsError::NotFound(name) => write!(f, "not found: {name}"),
            FsError::NotFoundAt(loc) => write!(f, "not found at dir entry: {loc}"),
            FsError::NotADirectory(name) => write!(f, "not a directory: {name}"),
            FsError::IsADirectory(name) => write!(f, "is a directory: {name}"),
            FsError::DirectoryNotEmpty(name) => write!(f, "directory not empty: {name}"),
            FsError::NoSpace => write!(f, "filesystem is full"),
            FsError::TooManyOpenFiles => write!(f, "too many opened files"),
            FsError::AlreadyOpen(loc) => write!(f, "file already open: {loc}"),
            FsError::FileOpen(loc) => write!(f, "file is open: {loc}"),
            FsError::InvalidHandle(handle) => write!(f, "invalid handle: {handle}"),
            FsError::SeekOutOfBounds(pos) => write!(f, "seek out of bounds: {pos}"),
            FsError::CorruptFs(msg) => write!(f, "corrupt filesystem: {msg}"),
        }
    }
}

impl error::Error for FsError {}

/// Configurable parameters of [`MyFileSystem`].
#[derive(Debug, Clone)]
pub struct FsConfig {
    pub block_size: u16,
    pub block_count: u16,
    pub blocks_per_cluster: u16,
}

impl Default for FsConfig {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE as u16,
            block_count: DEFAULT_BLOCK_COUNT,
            blocks_per_cluster: DEFAULT_BLOCKS_PER_CLUSTER,
        }
    }
}

impl FsConfig {
    pub fn validate(&self) -> Result<(), FsError> {
        if !(64..=1024).contains(&self.block_size) {
            return Err(FsError::InvalidConfig(format!(
                "block size {} must be between 64 and 1024",
                self.block_size
            )));
        }
        if (self.block_size as usize) < std::mem::size_of::<BootSector>() {
            return Err(FsError::InvalidConfig(format!(
                "boot sector does not fit in block size {}",
                self.block_size
            )));
        }
        if self.block_count <= 8 {
            return Err(FsError::InvalidConfig(format!(
                "block count {} is too small",
                self.block_count
            )));
        }
        if !self.block_size.is_multiple_of(64) {
            return Err(FsError::InvalidConfig(format!(
                "block size {} must be a multiple of 64",
                self.block_size
            )));
        }
        if self.blocks_per_cluster == 0 {
            return Err(FsError::InvalidConfig(
                "blocks per cluster must be at least 1".to_string(),
            ));
        }
        let min_blocks = 1 + 2 + self.blocks_per_cluster * ROOT_DIR_CLUSTER_COUNT;
        if self.block_count <= min_blocks {
            return Err(FsError::InvalidConfig(format!(
                "block count {} is too small for geometry",
                self.block_count
            )));
        }
        Ok(())
    }
}

/// Metadata of a node returned by [`MyFileSystem::stat_root`] and [`MyFileSystem::stat`].
#[derive(Debug, Clone)]
pub struct NodeMeta {
    pub loc: Option<DirEntryLoc>,
    pub short_name: String,
    pub kind: NodeKind,
    pub size: u32,
    pub start_cluster: ClusterId,
    pub ctime: u16,
    pub cdate: u16,
}

/// An entry of the directory returned by [`MyFileSystem::list_dir`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub loc: DirEntryLoc,
    pub short_name: String,
    pub kind: NodeKind,
    pub size: u32,
    pub start_cluster: ClusterId,
}

#[derive(Debug, Clone)]
enum DirSlot {
    Unused,
    Deleted,
    Occupied(Fcb),
}

impl TryFrom<&[u8]> for DirSlot {
    type Error = FsError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() < FCB_SIZE {
            return Err(FsError::CorruptFs(
                "fcb slot shorter than expected".to_string(),
            ));
        }
        match value[0] {
            Self::SLOT_UNUSED => Ok(DirSlot::Unused),
            Self::SLOT_DELETED => Ok(DirSlot::Deleted),
            _ => Ok(DirSlot::Occupied(Fcb::read_from_bytes(value)?)),
        }
    }
}

impl DirSlot {
    pub const SLOT_UNUSED: u8 = 0x00;
    pub const SLOT_DELETED: u8 = 0xE5;
}

/// State of an opened file.
#[derive(Debug, Clone)]
pub struct OpenFile {
    pub handle: FileHandle,
    pub loc: DirEntryLoc,
    pub cursor: usize,
    pub fcb: Fcb,
}

/// Main object for a mounted MyFileSystem instance.
pub struct MyFileSystem<D: BlockDevice> {
    boot: BootSector,
    device: D,
    open_files: [Option<OpenFile>; MAX_OPEN_FILES],
    next_handle: u32,
}

impl MyFileSystem<MemoryBlockDevice> {
    pub fn format_memory(config: FsConfig) -> Result<Self, FsError> {
        config.validate()?;

        let fat_block_count = get_fat_block_count(
            config.block_size,
            config.block_count,
            2,
            config.blocks_per_cluster,
        );
        let boot = BootSector {
            block_size: config.block_size,
            block_count: config.block_count,
            blocks_per_cluster: config.blocks_per_cluster,
            fat_start_block: BlockId(1),
            fat_block_count,
            fat_copies: 2,
            data_start_block: BlockId(1 + fat_block_count * 2),
            root_dir_start_cluster: ROOT_DIR_START_CLUSTER,
            root_dir_cluster_count: ROOT_DIR_CLUSTER_COUNT,
        };

        let mut fs = Self {
            device: MemoryBlockDevice::new(
                usize::from(config.block_size),
                usize::from(config.block_count),
            ),
            boot,
            open_files: std::array::from_fn(|_| None),
            next_handle: 1,
        };

        fs.write_boot_sector()?;
        fs.initialize_fat()?;
        fs.reserve_root_directory()?;
        Ok(fs)
    }
}

impl<D: BlockDevice> MyFileSystem<D> {
    pub fn boot_sector(&self) -> &BootSector {
        &self.boot
    }

    pub fn root_dir_cluster(&self) -> ClusterId {
        self.boot.root_dir_start_cluster
    }

    pub fn stat_root(&self) -> Result<NodeMeta, FsError> {
        Ok(NodeMeta {
            loc: None,
            short_name: "/".to_string(),
            kind: NodeKind::Directory,
            size: self.dir_size(self.root_dir_cluster())?,
            start_cluster: self.root_dir_cluster(),
            ctime: 0,
            cdate: 0,
        })
    }

    pub fn lookup(&self, parent_dir: ClusterId, name: &str) -> Result<(DirEntryLoc, Fcb), FsError> {
        let key = normalize_component(name)?;
        for (loc, slot) in self.scan_dir(parent_dir)? {
            if let DirSlot::Occupied(fcb) = slot
                && fcb.short_name() == key
            {
                return Ok((loc, fcb));
            }
        }
        Err(FsError::NotFound(format!("{parent_dir}/{key}")))
    }

    pub fn stat(&self, loc: DirEntryLoc) -> Result<NodeMeta, FsError> {
        let fcb = self.read_fcb_at(loc)?;
        let kind = fcb.kind()?;
        Ok(NodeMeta {
            loc: Some(loc),
            short_name: fcb.short_name(),
            kind,
            size: self.size_of(&fcb)?,
            start_cluster: fcb.start_cluster,
            ctime: fcb.ctime,
            cdate: fcb.cdate,
        })
    }

    pub fn list_dir(&self, dir_start: ClusterId) -> Result<Vec<DirEntry>, FsError> {
        let mut entries = Vec::new();
        for (loc, slot) in self.scan_dir(dir_start)? {
            if let DirSlot::Occupied(fcb) = slot {
                entries.push(DirEntry {
                    loc,
                    short_name: fcb.short_name(),
                    kind: fcb.kind()?,
                    size: self.size_of(&fcb)?,
                    start_cluster: fcb.start_cluster,
                });
            }
        }
        Ok(entries)
    }

    pub fn create_file(
        &mut self,
        parent_dir: ClusterId,
        name: &str,
    ) -> Result<DirEntryLoc, FsError> {
        let key = normalize_component(name)?;
        if self.lookup(parent_dir, &key).is_ok() {
            return Err(FsError::InvalidPath(format!("{key} already exists")));
        }
        let loc = self.find_free_dir_slot(parent_dir)?;
        let fcb = Fcb::new(&key, NodeKind::File, ClusterId::FREE, 0)?;
        self.write_fcb_at(loc, &fcb)?;
        self.update_dir_size_on_disk(parent_dir)?;
        Ok(loc)
    }

    pub fn mkdir(&mut self, parent_dir: ClusterId, name: &str) -> Result<DirEntryLoc, FsError> {
        let key = normalize_component(name)?;
        if self.lookup(parent_dir, &key).is_ok() {
            return Err(FsError::InvalidPath(format!("{key} already exists")));
        }
        let new_cluster = self.allocate_clusters(1)?[0];
        let loc = self.find_free_dir_slot(parent_dir)?;
        let fcb = Fcb::new(&key, NodeKind::Directory, new_cluster, 0)?;
        self.write_fcb_at(loc, &fcb)?;
        self.update_dir_size_on_disk(parent_dir)?;
        Ok(loc)
    }

    pub fn remove_file(&mut self, loc: DirEntryLoc) -> Result<(), FsError> {
        let fcb = self.read_fcb_at(loc)?;
        if fcb.kind()? == NodeKind::Directory {
            return Err(FsError::IsADirectory(fcb.short_name()));
        }
        if self.find_open_handle(loc).is_some() {
            return Err(FsError::FileOpen(loc));
        }
        self.free_chain_from(fcb.start_cluster)?;
        self.mark_slot_deleted(loc)?;
        self.update_dir_size_on_disk(loc.dir_start)?;
        Ok(())
    }

    pub fn rmdir(&mut self, loc: DirEntryLoc) -> Result<(), FsError> {
        let fcb = self.read_fcb_at(loc)?;
        if fcb.kind()? != NodeKind::Directory {
            return Err(FsError::NotADirectory(fcb.short_name()));
        }
        if !self.scan_dir(fcb.start_cluster)?.is_empty() {
            return Err(FsError::DirectoryNotEmpty(fcb.short_name()));
        }
        self.free_chain_from(fcb.start_cluster)?;
        self.mark_slot_deleted(loc)?;
        self.update_dir_size_on_disk(loc.dir_start)?;
        Ok(())
    }

    pub fn open(&mut self, loc: DirEntryLoc) -> Result<FileHandle, FsError> {
        let fcb = self.read_fcb_at(loc)?;
        if fcb.kind()? == NodeKind::Directory {
            return Err(FsError::IsADirectory(fcb.short_name()));
        }
        if self.find_open_handle(loc).is_some() {
            return Err(FsError::AlreadyOpen(loc));
        }

        let slot = self
            .open_files
            .iter()
            .position(Option::is_none)
            .ok_or(FsError::TooManyOpenFiles)?;
        let handle = FileHandle(self.next_handle);
        self.next_handle += 1;
        self.open_files[slot] = Some(OpenFile {
            handle,
            loc,
            cursor: 0,
            fcb,
        });
        Ok(handle)
    }

    pub fn close(&mut self, handle: FileHandle) -> Result<(), FsError> {
        let slot = self.find_open_slot(handle)?;
        self.open_files[slot] = None;
        Ok(())
    }

    pub fn find_open_handle(&self, loc: DirEntryLoc) -> Option<FileHandle> {
        self.open_files.iter().flatten().find_map(|entry| {
            if entry.loc == loc {
                Some(entry.handle)
            } else {
                None
            }
        })
    }

    pub fn opened_files(&self) -> Vec<OpenFile> {
        self.open_files.iter().flatten().cloned().collect()
    }

    pub fn seek(&mut self, handle: FileHandle, pos: usize) -> Result<(), FsError> {
        let slot = self.find_open_slot(handle)?;
        let file_size =
            usize::try_from(self.open_files[slot].as_ref().expect("open slot").fcb.size)
                .expect("file size must fit into usize");
        if pos > file_size {
            return Err(FsError::SeekOutOfBounds(pos));
        }
        self.open_files[slot].as_mut().expect("open slot").cursor = pos;
        Ok(())
    }

    pub fn read(&mut self, handle: FileHandle, len: usize) -> Result<Vec<u8>, FsError> {
        let slot = self.find_open_slot(handle)?;
        let open = self.open_files[slot].as_ref().expect("open slot").clone();
        let file_size = usize::try_from(open.fcb.size).expect("file size must fit into usize");
        let read_len = len.min(file_size.saturating_sub(open.cursor));
        let data = if open.fcb.start_cluster == ClusterId::FREE || read_len == 0 {
            Vec::new()
        } else {
            self.read_chain_bytes(open.fcb.start_cluster, open.cursor, read_len)?
        };
        self.open_files[slot].as_mut().expect("open slot").cursor += data.len();
        Ok(data)
    }

    pub fn write(&mut self, handle: FileHandle, data: &[u8]) -> Result<usize, FsError> {
        let slot = self.find_open_slot(handle)?;
        let open = self.open_files[slot].as_ref().expect("open slot").clone();
        let cursor = open.cursor;
        let new_end = cursor + data.len();
        let needed_clusters = if new_end == 0 {
            0
        } else {
            new_end.div_ceil(self.cluster_size())
        };

        let mut fcb = self.read_fcb_at(open.loc)?;
        fcb = self.ensure_fcb_capacity(fcb, needed_clusters)?;
        if !data.is_empty() {
            self.write_chain_bytes(fcb.start_cluster, cursor, data)?;
        }
        if new_end > usize::try_from(fcb.size).expect("file size must fit into usize") {
            fcb.size = u32::try_from(new_end).expect("file size exceeds u32 range");
        }
        self.write_fcb_at(open.loc, &fcb)?;

        let open_entry = self.open_files[slot].as_mut().expect("open slot");
        open_entry.cursor = new_end;
        open_entry.fcb = fcb;
        Ok(data.len())
    }

    pub fn dump_fat(&self) -> String {
        let mut out = String::new();
        for raw in u16::from(ROOT_DIR_START_CLUSTER)..=u16::from(self.max_cluster_id()) {
            let cluster = ClusterId::from(raw);
            let value = self.read_fat_entry(cluster).unwrap_or(ClusterId::FREE);
            out.push_str(&format!("{raw:>3} -> {value}\n"));
        }
        out
    }

    fn write_boot_sector(&mut self) -> Result<(), FsError> {
        let mut block = vec![0; self.device.block_size()];
        block[..BOOT_SECTOR_SIZE].copy_from_slice(self.boot.as_bytes());
        self.device.write_block(BlockId(0), &block);
        Ok(())
    }

    fn initialize_fat(&mut self) -> Result<(), FsError> {
        for block in 0..self.boot.fat_block_count {
            self.device
                .zero_block(BlockId::from(u16::from(self.boot.fat_start_block) + block));
            self.device.zero_block(BlockId::from(
                u16::from(self.boot.fat_start_block) + self.boot.fat_block_count + block,
            ));
        }
        Ok(())
    }

    fn reserve_root_directory(&mut self) -> Result<(), FsError> {
        for offset in 0..self.boot.root_dir_cluster_count {
            let cluster = ClusterId::from(u16::from(self.boot.root_dir_start_cluster) + offset);
            let next = if offset + 1 == self.boot.root_dir_cluster_count {
                ClusterId::EOC
            } else {
                ClusterId::from(u16::from(cluster) + 1)
            };
            self.write_fat_entry(cluster, next)?;
            self.zero_cluster(cluster)?;
        }
        Ok(())
    }

    fn size_of(&self, fcb: &Fcb) -> Result<u32, FsError> {
        if fcb.kind()? == NodeKind::Directory {
            self.dir_size(fcb.start_cluster)
        } else {
            Ok(fcb.size)
        }
    }

    fn dir_size(&self, dir_start: ClusterId) -> Result<u32, FsError> {
        Ok(self.scan_dir(dir_start)?.len() as u32 * FCB_SIZE as u32)
    }

    fn ensure_fcb_capacity(
        &mut self,
        mut fcb: Fcb,
        needed_clusters: usize,
    ) -> Result<Fcb, FsError> {
        let current = if fcb.start_cluster == ClusterId::FREE {
            0
        } else {
            self.cluster_chain(fcb.start_cluster)?.len()
        };
        if needed_clusters <= current {
            return Ok(fcb);
        }
        let extra = self.allocate_clusters(needed_clusters - current)?;
        if current == 0 {
            fcb.start_cluster = extra[0];
        } else {
            let chain = self.cluster_chain(fcb.start_cluster)?;
            let last = *chain.last().expect("existing chain has last cluster");
            self.write_fat_entry(last, extra[0])?;
        }
        for (idx, cluster) in extra.iter().enumerate() {
            let next = extra.get(idx + 1).copied().unwrap_or(ClusterId::EOC);
            self.write_fat_entry(*cluster, next)?;
        }
        Ok(fcb)
    }

    fn find_open_slot(&self, handle: FileHandle) -> Result<usize, FsError> {
        self.open_files
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|open| open.handle == handle))
            .ok_or(FsError::InvalidHandle(handle))
    }

    fn zero_cluster(&mut self, cluster: ClusterId) -> Result<(), FsError> {
        for block in self.cluster_blocks(cluster)? {
            self.device.zero_block(block);
        }
        Ok(())
    }

    fn cluster_size(&self) -> usize {
        usize::from(self.boot.block_size) * usize::from(self.boot.blocks_per_cluster)
    }

    fn data_cluster_count(&self) -> u16 {
        (self.boot.block_count - u16::from(self.boot.data_start_block))
            / self.boot.blocks_per_cluster
    }

    fn max_cluster_id(&self) -> ClusterId {
        ClusterId::from(u16::from(ROOT_DIR_START_CLUSTER) + self.data_cluster_count() - 1)
    }

    fn cluster_blocks(&self, cluster: ClusterId) -> Result<Vec<BlockId>, FsError> {
        if cluster < ROOT_DIR_START_CLUSTER || cluster > self.max_cluster_id() {
            return Err(FsError::CorruptFs(format!(
                "cluster {} outside data region",
                cluster
            )));
        }
        let first = u16::from(self.boot.data_start_block)
            + (u16::from(cluster) - u16::from(ROOT_DIR_START_CLUSTER))
                * self.boot.blocks_per_cluster;
        Ok((0..self.boot.blocks_per_cluster)
            .map(|offset| BlockId::from(first + offset))
            .collect())
    }

    fn fat_offset(&self, cluster: ClusterId) -> usize {
        usize::from(u16::from(cluster)) * 2
    }

    fn read_fat_entry(&self, cluster: ClusterId) -> Result<ClusterId, FsError> {
        let offset = self.fat_offset(cluster);
        let block_offset = offset / self.device.block_size();
        let byte_offset = offset % self.device.block_size();
        let block = BlockId::from(
            u16::from(self.boot.fat_start_block) + u16::try_from(block_offset).unwrap(),
        );
        let bytes = self.device.read_block(block);
        if byte_offset + 2 > bytes.len() {
            return Err(FsError::CorruptFs(
                "fat entry crosses block boundary".to_string(),
            ));
        }
        Ok(ClusterId::from(u16::from_le_bytes([
            bytes[byte_offset],
            bytes[byte_offset + 1],
        ])))
    }

    fn write_fat_entry(&mut self, cluster: ClusterId, value: ClusterId) -> Result<(), FsError> {
        let offset = self.fat_offset(cluster);
        let block_offset = offset / self.device.block_size();
        let byte_offset = offset % self.device.block_size();
        let fat_bytes = u16::from(value).to_le_bytes();

        for copy in 0..self.boot.fat_copies {
            let start = u16::from(self.boot.fat_start_block) + copy * self.boot.fat_block_count;
            let block = BlockId::from(start + u16::try_from(block_offset).unwrap());
            self.modify_block(block, |data| {
                data[byte_offset..byte_offset + 2].copy_from_slice(&fat_bytes);
            });
        }
        Ok(())
    }

    fn cluster_chain(&self, start: ClusterId) -> Result<Vec<ClusterId>, FsError> {
        if start == ClusterId::FREE {
            return Ok(Vec::new());
        }
        let mut chain = Vec::new();
        let mut current = start;
        loop {
            chain.push(current);
            let next = self.read_fat_entry(current)?;
            if next == ClusterId::EOC {
                break;
            }
            if next == ClusterId::FREE {
                return Err(FsError::CorruptFs(format!(
                    "cluster chain from {} reaches free entry",
                    start
                )));
            }
            if chain.contains(&next) {
                return Err(FsError::CorruptFs(format!(
                    "cluster loop detected at {}",
                    next
                )));
            }
            current = next;
        }
        Ok(chain)
    }

    fn allocate_clusters(&mut self, len: usize) -> Result<Vec<ClusterId>, FsError> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let cluster = (u16::from(ROOT_DIR_START_CLUSTER)..=u16::from(self.max_cluster_id()))
                .map(ClusterId::from)
                .find(|cluster| self.read_fat_entry(*cluster).ok() == Some(ClusterId::FREE))
                .ok_or(FsError::NoSpace)?;
            self.write_fat_entry(cluster, ClusterId::EOC)?;
            self.zero_cluster(cluster)?;
            out.push(cluster);
        }
        Ok(out)
    }

    fn free_chain_from(&mut self, start: ClusterId) -> Result<(), FsError> {
        if start == ClusterId::FREE {
            return Ok(());
        }
        let chain = self.cluster_chain(start)?;
        for cluster in chain {
            self.write_fat_entry(cluster, ClusterId::FREE)?;
            self.zero_cluster(cluster)?;
        }
        Ok(())
    }

    fn read_chain_bytes(
        &self,
        start: ClusterId,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, FsError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let cluster_size = self.cluster_size();
        let chain = self.cluster_chain(start)?;
        let mut out = Vec::with_capacity(len);
        let mut remaining = len;
        let mut cursor = offset;
        while remaining > 0 {
            let cluster_index = cursor / cluster_size;
            let cluster = *chain
                .get(cluster_index)
                .ok_or_else(|| FsError::CorruptFs("offset beyond cluster chain".to_string()))?;
            let cluster_bytes = self.read_cluster_bytes(cluster)?;
            let in_cluster = cursor % cluster_size;
            let chunk = remaining.min(cluster_size - in_cluster);
            out.extend_from_slice(&cluster_bytes[in_cluster..in_cluster + chunk]);
            cursor += chunk;
            remaining -= chunk;
        }
        Ok(out)
    }

    fn write_chain_bytes(
        &mut self,
        start: ClusterId,
        offset: usize,
        data: &[u8],
    ) -> Result<(), FsError> {
        if data.is_empty() {
            return Ok(());
        }
        let cluster_size = self.cluster_size();
        let chain = self.cluster_chain(start)?;
        let mut remaining = data.len();
        let mut cursor = offset;
        let mut written = 0;
        while remaining > 0 {
            let cluster_index = cursor / cluster_size;
            let cluster = *chain
                .get(cluster_index)
                .ok_or_else(|| FsError::CorruptFs("offset beyond cluster chain".to_string()))?;
            let mut cluster_bytes = self.read_cluster_bytes(cluster)?;
            let in_cluster = cursor % cluster_size;
            let chunk = remaining.min(cluster_size - in_cluster);
            cluster_bytes[in_cluster..in_cluster + chunk]
                .copy_from_slice(&data[written..written + chunk]);
            self.write_cluster_bytes(cluster, &cluster_bytes)?;
            cursor += chunk;
            written += chunk;
            remaining -= chunk;
        }
        Ok(())
    }

    fn read_cluster_bytes(&self, cluster: ClusterId) -> Result<Vec<u8>, FsError> {
        let mut out = Vec::with_capacity(self.cluster_size());
        for block in self.cluster_blocks(cluster)? {
            out.extend_from_slice(self.device.read_block(block));
        }
        Ok(out)
    }

    fn write_cluster_bytes(&mut self, cluster: ClusterId, data: &[u8]) -> Result<(), FsError> {
        if data.len() != self.cluster_size() {
            return Err(FsError::CorruptFs(
                "cluster write size mismatch".to_string(),
            ));
        }
        for (idx, block) in self.cluster_blocks(cluster)?.into_iter().enumerate() {
            let start = idx * usize::from(self.boot.block_size);
            let end = start + usize::from(self.boot.block_size);
            self.device.write_block(block, &data[start..end]);
        }
        Ok(())
    }

    fn scan_dir(&self, dir_start: ClusterId) -> Result<Vec<(DirEntryLoc, DirSlot)>, FsError> {
        let mut out = Vec::new();
        let chain = self.cluster_chain(dir_start)?;
        let entries_per_cluster = self.cluster_size() / FCB_SIZE;
        let mut entry_index = 0u32;
        for cluster in chain {
            let bytes = self.read_cluster_bytes(cluster)?;
            for slot in 0..entries_per_cluster {
                let start = slot * FCB_SIZE;
                let end = start + FCB_SIZE;
                let parsed = DirSlot::try_from(&bytes[start..end])?;
                match parsed {
                    DirSlot::Unused => {}
                    DirSlot::Deleted => {}
                    DirSlot::Occupied(_) => out.push((
                        DirEntryLoc {
                            dir_start,
                            entry_index,
                        },
                        parsed,
                    )),
                }
                entry_index += 1;
            }
        }
        Ok(out)
    }

    fn read_fcb_at(&self, loc: DirEntryLoc) -> Result<Fcb, FsError> {
        match self.read_slot(loc)? {
            DirSlot::Occupied(fcb) => Ok(fcb),
            _ => Err(FsError::NotFoundAt(loc)),
        }
    }

    fn read_slot(&self, loc: DirEntryLoc) -> Result<DirSlot, FsError> {
        let bytes = self.read_chain_bytes(loc.dir_start, self.slot_offset(loc), FCB_SIZE)?;
        DirSlot::try_from(bytes.as_slice())
    }

    fn write_fcb_at(&mut self, loc: DirEntryLoc, fcb: &Fcb) -> Result<(), FsError> {
        let mut bytes = [0; FCB_SIZE];
        fcb.write_to_slice(&mut bytes)?;
        self.write_chain_bytes(loc.dir_start, self.slot_offset(loc), &bytes)
    }

    fn mark_slot_deleted(&mut self, loc: DirEntryLoc) -> Result<(), FsError> {
        let mut bytes = [0; FCB_SIZE];
        bytes[0] = DirSlot::SLOT_DELETED;
        self.write_chain_bytes(loc.dir_start, self.slot_offset(loc), &bytes)
    }

    /// Perform a read-modify-write procedure.
    fn modify_block(&mut self, block: BlockId, f: impl FnOnce(&mut [u8])) {
        let mut bytes = self.device.read_block(block).to_vec();
        f(&mut bytes);
        self.device.write_block(block, &bytes);
    }

    fn slot_offset(&self, loc: DirEntryLoc) -> usize {
        usize::try_from(loc.entry_index).unwrap() * FCB_SIZE
    }

    fn find_free_dir_slot(&mut self, dir_start: ClusterId) -> Result<DirEntryLoc, FsError> {
        let chain = self.cluster_chain(dir_start)?;
        let entries_per_cluster = self.cluster_size() / FCB_SIZE;
        let mut entry_index = 0u32;
        for cluster in &chain {
            let bytes = self.read_cluster_bytes(*cluster)?;
            for slot in 0..entries_per_cluster {
                let start = slot * FCB_SIZE;
                let end = start + FCB_SIZE;
                match DirSlot::try_from(&bytes[start..end])? {
                    DirSlot::Unused | DirSlot::Deleted => {
                        return Ok(DirEntryLoc {
                            dir_start,
                            entry_index,
                        });
                    }
                    DirSlot::Occupied(_) => {}
                }
                entry_index += 1;
            }
        }

        let new_cluster = self.allocate_clusters(1)?[0];
        let last = *chain
            .last()
            .ok_or_else(|| FsError::CorruptFs("directory has no cluster chain".to_string()))?;
        self.write_fat_entry(last, new_cluster)?;
        self.write_fat_entry(new_cluster, ClusterId::EOC)?;
        Ok(DirEntryLoc {
            dir_start,
            entry_index,
        })
    }

    fn update_dir_size_on_disk(&mut self, dir_start: ClusterId) -> Result<(), FsError> {
        if dir_start == self.root_dir_cluster() {
            return Ok(());
        }
        let size = self.dir_size(dir_start)?;
        if let Some(loc) = self.find_dir_loc(self.root_dir_cluster(), dir_start)? {
            let mut fcb = self.read_fcb_at(loc)?;
            fcb.size = size;
            self.write_fcb_at(loc, &fcb)?;
        }
        Ok(())
    }

    fn find_dir_loc(
        &self,
        dir_start: ClusterId,
        target: ClusterId,
    ) -> Result<Option<DirEntryLoc>, FsError> {
        for (loc, slot) in self.scan_dir(dir_start)? {
            if let DirSlot::Occupied(fcb) = slot
                && fcb.kind()? == NodeKind::Directory
            {
                if fcb.start_cluster == target {
                    return Ok(Some(loc));
                }
                if let Some(found) = self.find_dir_loc(fcb.start_cluster, target)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }
}

fn get_fat_block_count(
    block_size: u16,
    block_count: u16,
    fat_copies: u16,
    blocks_per_cluster: u16,
) -> u16 {
    let mut fat_blocks = 1u16;
    loop {
        let data_start = 1 + fat_copies * fat_blocks;
        if data_start >= block_count {
            return fat_blocks;
        }
        let data_clusters = (block_count - data_start) / blocks_per_cluster;
        let fat_entries = usize::from(u16::from(ROOT_DIR_START_CLUSTER) + data_clusters);
        let fat_bytes = fat_entries * 2;
        let needed = fat_bytes.div_ceil(usize::from(block_size)) as u16;
        if needed == fat_blocks {
            return fat_blocks;
        }
        fat_blocks = needed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkmemfs() -> MyFileSystem<MemoryBlockDevice> {
        MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig::default())
            .expect("filesystem should format")
    }

    #[test]
    fn parse_dir_slot_recognizes_unused_and_deleted() {
        let unused = [0u8; FCB_SIZE];
        assert!(matches!(
            DirSlot::try_from(unused.as_slice()).unwrap(),
            DirSlot::Unused
        ));

        let mut deleted = [0u8; FCB_SIZE];
        deleted[0] = DirSlot::SLOT_DELETED;
        assert!(matches!(
            DirSlot::try_from(deleted.as_slice()).unwrap(),
            DirSlot::Deleted
        ));
    }

    #[test]
    fn fs_config_validation_rejects_bad_values() {
        assert!(
            FsConfig {
                block_size: 63,
                block_count: 128,
                blocks_per_cluster: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 128,
                block_count: 8,
                blocks_per_cluster: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 96,
                block_count: 128,
                blocks_per_cluster: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 128,
                block_count: 128,
                blocks_per_cluster: 0,
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 128,
                block_count: 128,
                blocks_per_cluster: 16,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn format_writes_boot_and_two_fat_copies() {
        let fs = mkmemfs();
        let boot_block = fs.device.read_block(BlockId(0));
        assert_eq!(
            u16::from_le_bytes([boot_block[0], boot_block[1]]),
            DEFAULT_BLOCK_SIZE as u16
        );

        let fat1 = fs.device.read_block(BlockId(1)).to_vec();
        let fat2 = fs.device.read_block(BlockId(2)).to_vec();
        assert_eq!(fat1, fat2);
        assert_eq!(
            fs.read_fat_entry(ROOT_DIR_START_CLUSTER).unwrap(),
            ClusterId(3)
        );
        assert_eq!(fs.read_fat_entry(ClusterId(3)).unwrap(), ClusterId::EOC);
    }

    #[test]
    fn format_respects_blocks_per_cluster() {
        let fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 128,
            block_count: 256,
            blocks_per_cluster: 4,
        })
        .unwrap();
        assert_eq!(fs.boot.blocks_per_cluster, 4);
        assert_eq!(fs.cluster_size(), 512);
        assert_eq!(fs.cluster_blocks(ROOT_DIR_START_CLUSTER).unwrap().len(), 4);
    }

    #[test]
    fn creates_lists_and_removes_directories() {
        let mut fs = mkmemfs();
        let docs_loc = fs.mkdir(fs.root_dir_cluster(), "DOCS").unwrap();
        let docs_cluster = fs.read_fcb_at(docs_loc).unwrap().start_cluster;
        let readme_loc = fs.create_file(docs_cluster, "README.TXT").unwrap();

        let root = fs.list_dir(fs.root_dir_cluster()).unwrap();
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].short_name, "DOCS");
        assert_eq!(root[0].kind, NodeKind::Directory);
        assert_eq!(root[0].loc, docs_loc);

        let docs = fs.list_dir(docs_cluster).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].short_name, "README.TXT");
        assert_eq!(docs[0].loc, readme_loc);

        assert!(matches!(
            fs.rmdir(docs_loc),
            Err(FsError::DirectoryNotEmpty(_))
        ));

        fs.remove_file(readme_loc).unwrap();
        fs.rmdir(docs_loc).unwrap();
        assert!(fs.list_dir(fs.root_dir_cluster()).unwrap().is_empty());
    }

    #[test]
    fn writes_reads_and_seeks_across_clusters() {
        let mut fs = mkmemfs();
        let file_loc = fs.create_file(fs.root_dir_cluster(), "DATA.BIN").unwrap();
        let handle = fs.open(file_loc).unwrap();
        let payload = [0xAB; DEFAULT_BLOCK_SIZE + 200];
        let written = fs.write(handle, &payload).unwrap();
        assert_eq!(written, payload.len());
        fs.seek(handle, 0).unwrap();
        let read_back = fs.read(handle, payload.len()).unwrap();
        assert_eq!(read_back, payload);
        fs.close(handle).unwrap();

        let stat = fs.stat(file_loc).unwrap();
        assert_eq!(stat.size, payload.len() as u32);
        assert_ne!(stat.start_cluster, ClusterId::FREE);
        let start = fs.read_fcb_at(file_loc).unwrap().start_cluster;
        assert_eq!(fs.cluster_chain(start).unwrap().len(), 2);
    }

    #[test]
    fn supports_lookup_and_root_stat_from_disk() {
        let mut fs = mkmemfs();
        let docs_loc = fs.mkdir(fs.root_dir_cluster(), "DOCS").unwrap();
        let docs_fcb = fs.read_fcb_at(docs_loc).unwrap();
        let readme_loc = fs.create_file(docs_fcb.start_cluster, "A.TXT").unwrap();

        let root_meta = fs.stat_root().unwrap();
        assert_eq!(root_meta.kind, NodeKind::Directory);
        assert_eq!(root_meta.start_cluster, ROOT_DIR_START_CLUSTER);

        let (found_docs, _) = fs.lookup(fs.root_dir_cluster(), "DOCS").unwrap();
        let (found_readme, _) = fs.lookup(docs_fcb.start_cluster, "A.TXT").unwrap();
        assert_eq!(found_docs, docs_loc);
        assert_eq!(found_readme, readme_loc);
    }

    #[test]
    fn enforces_open_file_rules() {
        let mut fs = mkmemfs();
        let file_loc = fs.create_file(fs.root_dir_cluster(), "ONE.TXT").unwrap();
        let handle = fs.open(file_loc).unwrap();
        assert!(matches!(fs.open(file_loc), Err(FsError::AlreadyOpen(_))));
        assert!(matches!(
            fs.remove_file(file_loc),
            Err(FsError::FileOpen(_))
        ));
        fs.close(handle).unwrap();
        fs.remove_file(file_loc).unwrap();

        for idx in 0..MAX_OPEN_FILES {
            let loc = fs
                .create_file(fs.root_dir_cluster(), &format!("F{idx}.TXT"))
                .unwrap();
            fs.open(loc).unwrap();
        }
        let last_loc = fs.create_file(fs.root_dir_cluster(), "LAST.TXT").unwrap();
        assert!(matches!(fs.open(last_loc), Err(FsError::TooManyOpenFiles)));
    }
}
