#![allow(unused_imports)]

mod dev;
mod fat;
mod fs;
mod name;

pub use dev::*;
pub use fat::*;
pub use fs::*;
pub use name::*;

use chrono::DateTime;
use chrono::NaiveDate;
use chrono::NaiveDateTime;
use chrono::NaiveTime;
use chrono::Utc;
use log::debug;
use log::trace;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fmt;
use thiserror::Error;

pub const MAX_OPEN_FILES: usize = 10;

/// Error type for [`MyFileSystem`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FsError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("not found at dir entry: {0}")]
    NotFoundAt(DirEntryLoc),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("is a directory: {0}")]
    IsADirectory(String),
    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),
    #[error("filesystem is full")]
    NoSpace,
    #[error("too many opened files")]
    TooManyOpenFiles,
    #[error("file already open: {0}")]
    AlreadyOpen(DirEntryLoc),
    #[error("file is open: {0}")]
    FileOpen(DirEntryLoc),
    #[error("invalid handle: {0}")]
    InvalidHandle(FileHandle),
    #[error("seek out of bounds: {0}")]
    SeekOutOfBounds(usize),
    #[error("corrupt filesystem: {0}")]
    CorruptFs(String),
}

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

/// Session-stable node identifier for higher-level directory traversal APIs.
///
/// This is comprised of two parts, both coming from [`DirEntryLoc`]:
/// * Lower 32 bits: `entry_index`
/// * Higher 32 bits: `dir_start`
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(u64);

impl NodeId {
    pub const ROOT: Self = Self(1);
}

impl From<u64> for NodeId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<NodeId> for u64 {
    fn from(value: NodeId) -> Self {
        value.0
    }
}

impl From<DirEntryLoc> for NodeId {
    fn from(value: DirEntryLoc) -> Self {
        Self((u64::from(u16::from(value.dir_start)) << 32) | u64::from(value.entry_index))
    }
}

/// The result for creating a [`DirEntryLoc`] from a [`NodeId`].
///
/// For internal use only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirEntryLocFromNodeIdResult {
    Root,
    Leaf(DirEntryLoc),
}

impl From<NodeId> for DirEntryLocFromNodeIdResult {
    fn from(value: NodeId) -> Self {
        if value == NodeId::ROOT {
            DirEntryLocFromNodeIdResult::Root
        } else {
            DirEntryLocFromNodeIdResult::Leaf(DirEntryLoc {
                dir_start: ClusterId::from((value.0 >> 32) as u16),
                entry_index: value.0 as u32,
            })
        }
    }
}

impl TryFrom<NodeId> for DirEntryLoc {
    type Error = NodeId;

    fn try_from(value: NodeId) -> Result<Self, Self::Error> {
        match DirEntryLocFromNodeIdResult::from(value) {
            DirEntryLocFromNodeIdResult::Root => Err(value),
            DirEntryLocFromNodeIdResult::Leaf(loc) => Ok(loc),
        }
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::ROOT {
            write!(f, "<root>")
        } else {
            write!(f, "{:x}", self.0)
        }
    }
}

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
        if self.block_size == 0 {
            return Err(FsError::InvalidConfig(
                "block size must be at least 1".to_string(),
            ));
        }
        if !self.block_size.is_power_of_two() {
            return Err(FsError::InvalidConfig(format!(
                "block size {} is not a power of 2",
                self.block_size
            )));
        }
        if (self.block_size as usize) < std::mem::size_of::<BootSector>() {
            return Err(FsError::InvalidConfig(format!(
                "boot sector does not fit in block size {}",
                self.block_size
            )));
        }
        if self.blocks_per_cluster == 0 {
            return Err(FsError::InvalidConfig(
                "blocks per cluster must be at least 1".to_string(),
            ));
        }
        if !self.blocks_per_cluster.is_power_of_two() {
            return Err(FsError::InvalidConfig(format!(
                "blocks per cluster {} is not a power of 2",
                self.blocks_per_cluster
            )));
        }
        let cluster_size =
            usize::from(self.block_size).saturating_mul(usize::from(self.blocks_per_cluster));
        if cluster_size < Fcb::SIZE {
            return Err(FsError::InvalidConfig(format!(
                "cluster size {} smaller than directory entry size {}",
                cluster_size,
                Fcb::SIZE
            )));
        }

        let fat_block_count = get_fat_block_count(
            self.block_size,
            self.block_count,
            2,
            self.blocks_per_cluster,
        );
        let data_start = 1u32 + 2u32 * u32::from(fat_block_count);
        if data_start >= u32::from(self.block_count) {
            return Err(FsError::InvalidConfig(format!(
                "block count {} is too small for geometry",
                self.block_count
            )));
        }
        let data_blocks = u32::from(self.block_count) - data_start;
        if data_blocks < u32::from(self.blocks_per_cluster) {
            return Err(FsError::InvalidConfig(
                "geometry does not leave one full root directory cluster".to_string(),
            ));
        }

        let data_clusters = data_blocks / u32::from(self.blocks_per_cluster);
        let max_cluster_id = u32::from(u16::from(ROOT_DIR_START_CLUSTER)) + data_clusters - 1;
        if max_cluster_id > u32::from(u16::MAX) {
            return Err(FsError::InvalidConfig(
                "geometry exceeds 16-bit cluster id range".to_string(),
            ));
        }

        let fat_entries = fat_entry_count(self.block_size, fat_block_count);
        let needed_fat_entries = usize::from(u16::from(ROOT_DIR_START_CLUSTER))
            + usize::try_from(data_clusters).unwrap();
        if fat_entries < needed_fat_entries {
            return Err(FsError::InvalidConfig(
                "fat region cannot address all data clusters".to_string(),
            ));
        }
        Ok(())
    }
}

/// Metadata of a node returned by [`MyFileSystem::stat_root`] and [`MyFileSystem::stat`].
#[derive(Debug, Clone)]
pub struct NodeMeta {
    pub node_id: NodeId,
    pub loc: Option<DirEntryLoc>,
    pub short_name: String,
    pub kind: NodeKind,
    pub size: u32,
    pub start_cluster: ClusterId,
    pub mtime: U16Time,
    pub mdate: U16Date,
}

/// An entry yielded by [`DirEntryIter`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub node_id: NodeId,
    pub loc: DirEntryLoc,
    pub short_name: String,
    pub kind: NodeKind,
    pub size: u32,
    pub start_cluster: ClusterId,
    pub mdatetime: NaiveDateTime,
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
        if value.len() < Fcb::SIZE {
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

struct ChainIter<'a, D: BufferedBlockDevice> {
    fs: &'a MyFileSystem<D>,
    start: ClusterId,
    current: Option<ClusterId>,
    visited: HashSet<ClusterId>,
}

impl<'a, D: BufferedBlockDevice> ChainIter<'a, D> {
    fn new(fs: &'a MyFileSystem<D>, start: ClusterId) -> Result<Self, FsError> {
        if start != ClusterId::FREE {
            let _ = fs.fat_pos_of(start)?;
        }
        Ok(Self {
            fs,
            start,
            current: (start != ClusterId::FREE).then_some(start),
            visited: HashSet::new(),
        })
    }
}

impl<'a, D: BufferedBlockDevice> Iterator for ChainIter<'a, D> {
    type Item = Result<ClusterId, FsError>;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.current?;
        trace!(
            "chain iterator step. start={}, current={}",
            self.start, current
        );
        if !self.visited.insert(current) {
            self.current = None;
            return Some(Err(FsError::CorruptFs(format!(
                "cluster loop detected at {}",
                current
            ))));
        }

        match self.fs.read_fat(current) {
            Ok(FatEntry::Free) => {
                trace!(
                    "chain iterator reached free entry. start={}, current={}",
                    self.start, current
                );
                self.current = None;
                Some(Err(FsError::CorruptFs(format!(
                    "cluster chain from {} reaches free entry",
                    self.start
                ))))
            }
            Ok(FatEntry::EndOfChain) => {
                trace!(
                    "chain iterator reached end. start={}, current={}",
                    self.start, current
                );
                self.current = None;
                Some(Ok(current))
            }
            Ok(FatEntry::Next(next)) => {
                trace!("chain iterator advance. current={}, next={}", current, next);
                self.current = Some(next);
                Some(Ok(current))
            }
            Err(err) => {
                trace!(
                    "chain iterator error. start={}, current={}, err={}",
                    self.start, current, err
                );
                self.current = None;
                Some(Err(err))
            }
        }
    }
}

struct RawDirSlotIter<'a, D: BufferedBlockDevice> {
    fs: &'a MyFileSystem<D>,
    dir_start: ClusterId,
    chain_iter: ChainIter<'a, D>,
    current_bytes: Option<Vec<u8>>,
    slot_in_cluster: usize,
    entry_index: u32,
    entries_per_cluster: usize,
}

impl<'a, D: BufferedBlockDevice> RawDirSlotIter<'a, D> {
    fn new(fs: &'a MyFileSystem<D>, dir_start: ClusterId) -> Result<Self, FsError> {
        let entries_per_cluster = fs.cluster_size() / Fcb::SIZE;
        trace!(
            "raw_dir_slots(dir_start={}), entries_per_cluster={}",
            dir_start, entries_per_cluster
        );
        Ok(Self {
            fs,
            dir_start,
            chain_iter: ChainIter::new(fs, dir_start)?,
            current_bytes: None,
            slot_in_cluster: 0,
            entry_index: 0,
            entries_per_cluster,
        })
    }
}

impl<'a, D: BufferedBlockDevice> Iterator for RawDirSlotIter<'a, D> {
    type Item = Result<(DirEntryLoc, DirSlot), FsError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_bytes.is_none() || self.slot_in_cluster == self.entries_per_cluster {
            let cluster = match self.chain_iter.next()? {
                Ok(cluster) => cluster,
                Err(err) => return Some(Err(err)),
            };
            trace!(
                "scan directory cluster. dir_start={}, cluster={}",
                self.dir_start, cluster
            );
            let bytes = match self.fs.read_cluster_bytes(cluster) {
                Ok(bytes) => bytes,
                Err(err) => return Some(Err(err)),
            };
            self.current_bytes = Some(bytes);
            self.slot_in_cluster = 0;
        }

        let start = self.slot_in_cluster * Fcb::SIZE;
        let end = start + Fcb::SIZE;
        let loc = DirEntryLoc {
            dir_start: self.dir_start,
            entry_index: self.entry_index,
        };
        self.slot_in_cluster += 1;
        self.entry_index += 1;
        let bytes = self
            .current_bytes
            .as_ref()
            .expect("current directory cluster");
        Some(DirSlot::try_from(&bytes[start..end]).map(|slot| (loc, slot)))
    }
}

pub struct DirEntryIter<'a, D: BufferedBlockDevice> {
    fs: &'a MyFileSystem<D>,
    inner: RawDirSlotIter<'a, D>,
}

impl<'a, D: BufferedBlockDevice> DirEntryIter<'a, D> {
    fn new(fs: &'a MyFileSystem<D>, dir_start: ClusterId) -> Result<Self, FsError> {
        Ok(Self {
            fs,
            inner: RawDirSlotIter::new(fs, dir_start)?,
        })
    }

    fn dir_entry_from_fcb(&self, loc: DirEntryLoc, fcb: Fcb) -> Result<DirEntry, FsError> {
        let mdate = NaiveDate::try_from(fcb.mdate)?;
        let mtime = NaiveTime::try_from(fcb.mtime)?;
        Ok(DirEntry {
            node_id: loc.into(),
            loc,
            short_name: fcb.short_name(),
            kind: fcb.kind()?,
            size: self.fs.size_of(&fcb)?,
            start_cluster: fcb.start_cluster,
            mdatetime: NaiveDateTime::new(mdate, mtime),
        })
    }
}

impl<'a, D: BufferedBlockDevice> Iterator for DirEntryIter<'a, D> {
    type Item = Result<DirEntry, FsError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (loc, slot) = match self.inner.next()? {
                Ok(value) => value,
                Err(err) => return Some(Err(err)),
            };
            let DirSlot::Occupied(fcb) = slot else {
                continue;
            };
            return Some(self.dir_entry_from_fcb(loc, fcb));
        }
    }
}

/// Main object for a mounted MyFileSystem instance.
pub struct MyFileSystem<D: BufferedBlockDevice> {
    boot: BootSector,
    device: RefCell<D>,
    fat_m: Vec<FatEntry>,
    fat_dirty: bool,
    open_files: [Option<OpenFile>; MAX_OPEN_FILES],
    next_handle: u32,
}

impl MyFileSystem<MemoryBlockDevice> {
    pub fn format_memory(config: FsConfig) -> Result<Self, FsError> {
        let device = MemoryBlockDevice::new(
            usize::from(config.block_size),
            usize::from(config.block_count),
        );
        Self::format_on_device(device, config)
    }
}

impl<D: BufferedBlockDevice> MyFileSystem<D> {
    pub fn format_on_device(device: D, config: FsConfig) -> Result<Self, FsError> {
        config.validate()?;
        if device.block_size() != usize::from(config.block_size) {
            return Err(FsError::InvalidConfig(format!(
                "device block size {} does not match filesystem block size {}",
                device.block_size(),
                config.block_size
            )));
        }
        if device.block_count() < usize::from(config.block_count) {
            return Err(FsError::InvalidConfig(format!(
                "filesystem block count {} exceeds device block count {}",
                config.block_count,
                device.block_count(),
            )));
        }

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
        };
        debug!(
            "format_on_device(block_size={}, block_count={}, blocks_per_cluster={}), fat_blocks={}",
            config.block_size, config.block_count, config.blocks_per_cluster, fat_block_count
        );

        let mut fs = Self {
            device: RefCell::new(device),
            boot,
            fat_m: vec![FatEntry::Free; fat_entry_count(config.block_size, fat_block_count)],
            fat_dirty: false,
            open_files: std::array::from_fn(|_| None),
            next_handle: 1,
        };

        fs.write_boot_sector()?;
        fs.init_fat()?;
        fs.init_root_dir()?;
        fs.sync()?;
        Ok(fs)
    }

    pub fn open_on_device(mut device: D) -> Result<Self, FsError> {
        let boot = read_boot_sector_from_device(&mut device)?;
        validate_boot_sector_against_device(&boot, &device)?;
        let fat_m = read_fat_cache_from_device(&mut device, &boot)?;

        debug!(
            "open_on_device(block_size={}, block_count={}, blocks_per_cluster={}), fat_blocks={}",
            boot.block_size, boot.block_count, boot.blocks_per_cluster, boot.fat_block_count
        );

        Ok(Self {
            boot,
            device: RefCell::new(device),
            fat_m,
            fat_dirty: false,
            open_files: std::array::from_fn(|_| None),
            next_handle: 1,
        })
    }

    pub fn boot_sector(&self) -> &BootSector {
        &self.boot
    }

    pub fn root_dir_cluster(&self) -> ClusterId {
        self.boot.root_dir_start_cluster
    }

    pub fn root_node(&self) -> NodeId {
        NodeId::ROOT
    }

    pub fn stat_root(&self) -> Result<NodeMeta, FsError> {
        Ok(NodeMeta {
            node_id: NodeId::ROOT,
            loc: None,
            short_name: "/".to_string(),
            kind: NodeKind::Directory,
            size: self.dir_size(self.root_dir_cluster())?,
            start_cluster: self.root_dir_cluster(),
            mtime: U16Time::EMPTY,
            mdate: U16Date::EMPTY,
        })
    }

    pub fn lookup(&self, parent_dir: ClusterId, name: &str) -> Result<(DirEntryLoc, Fcb), FsError> {
        let key = normalize_component(name)?;
        debug!("lookup(parent_dir={}, name={})", parent_dir, key);
        for slot in self.raw_dir_slots(parent_dir)? {
            let (loc, slot) = slot?;
            if let DirSlot::Occupied(fcb) = slot
                && fcb.short_name() == key
            {
                debug!("lookup hit. loc={}", loc);
                return Ok((loc, fcb));
            }
        }
        Err(FsError::NotFound(format!("{parent_dir}/{key}")))
    }

    pub fn stat(&self, loc: DirEntryLoc) -> Result<NodeMeta, FsError> {
        let fcb = self.read_fcb_at(loc)?;
        self.node_meta_from_fcb(loc, fcb)
    }

    pub fn dir_entries(&self, dir_start: ClusterId) -> Result<DirEntryIter<'_, D>, FsError> {
        DirEntryIter::new(self, dir_start)
    }

    pub fn lookup_node(&self, parent: NodeId, name: &str) -> Result<NodeId, FsError> {
        let parent_cluster = self.node_directory_cluster(parent)?;
        let (loc, _) = self.lookup(parent_cluster, name)?;
        Ok(loc.into())
    }

    pub fn stat_node(&self, node_id: NodeId) -> Result<NodeMeta, FsError> {
        match node_id.into() {
            DirEntryLocFromNodeIdResult::Root => self.stat_root(),
            DirEntryLocFromNodeIdResult::Leaf(loc) => self.stat(loc),
        }
    }

    pub fn dir_entries_node(&self, node_id: NodeId) -> Result<DirEntryIter<'_, D>, FsError> {
        let dir_cluster = self.node_directory_cluster(node_id)?;
        self.dir_entries(dir_cluster)
    }

    pub fn open_node(&mut self, node_id: NodeId) -> Result<FileHandle, FsError> {
        let loc = match node_id.into() {
            DirEntryLocFromNodeIdResult::Root => Err(FsError::IsADirectory("/".to_string())),
            DirEntryLocFromNodeIdResult::Leaf(loc) => Ok(loc),
        }?;
        self.open(loc)
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
        let fcb = Fcb::new(&key, NodeKind::File, ClusterId::FREE, 0, Utc::now())?;
        self.write_fcb_at(loc, &fcb)?;
        self.update_dir_size_on_disk(parent_dir)?;
        debug!(
            "create_file(parent_dir={}, name={}), loc={}",
            parent_dir, key, loc
        );
        Ok(loc)
    }

    pub fn mkdir(&mut self, parent_dir: ClusterId, name: &str) -> Result<DirEntryLoc, FsError> {
        let key = normalize_component(name)?;
        if self.lookup(parent_dir, &key).is_ok() {
            return Err(FsError::InvalidPath(format!("{key} already exists")));
        }
        let new_cluster = self.allocate_clusters(1)?[0];
        let loc = self.find_free_dir_slot(parent_dir)?;
        let fcb = Fcb::new(&key, NodeKind::Directory, new_cluster, 0, Utc::now())?;
        self.write_fcb_at(loc, &fcb)?;
        self.update_dir_size_on_disk(parent_dir)?;
        debug!(
            "mkdir(parent_dir={}, name={}), loc={}, start_cluster={}",
            parent_dir, key, loc, new_cluster
        );
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
        debug!("remove_file(loc={})", loc);
        Ok(())
    }

    pub fn rmdir(&mut self, loc: DirEntryLoc) -> Result<(), FsError> {
        let fcb = self.read_fcb_at(loc)?;
        if fcb.kind()? != NodeKind::Directory {
            return Err(FsError::NotADirectory(fcb.short_name()));
        }
        for slot in self.raw_dir_slots(fcb.start_cluster)? {
            if matches!(slot?.1, DirSlot::Occupied(_)) {
                return Err(FsError::DirectoryNotEmpty(fcb.short_name()));
            }
        }
        self.free_chain_from(fcb.start_cluster)?;
        self.mark_slot_deleted(loc)?;
        self.update_dir_size_on_disk(loc.dir_start)?;
        debug!("rmdir(loc={})", loc);
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
        self.next_handle = self.next_handle.wrapping_add(1);
        self.open_files[slot] = Some(OpenFile {
            handle,
            loc,
            cursor: 0,
            fcb,
        });
        debug!("open(loc={}), handle={}", loc, handle);
        Ok(handle)
    }

    pub fn close(&mut self, handle: FileHandle) -> Result<(), FsError> {
        let slot = self.find_open_slot(handle)?;
        self.open_files[slot] = None;
        debug!("close(handle={})", handle);
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

    pub fn open_files(&self) -> impl Iterator<Item = &OpenFile> + '_ {
        self.open_files.iter().flatten()
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
        debug!("seek(handle={}, pos={})", handle, pos);
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

    pub fn read_file_at(
        &self,
        loc: DirEntryLoc,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, FsError> {
        let fcb = self.read_fcb_at(loc)?;
        if fcb.kind()? == NodeKind::Directory {
            return Err(FsError::IsADirectory(fcb.short_name()));
        }
        let file_size = usize::try_from(fcb.size).expect("file size must fit into usize");
        let read_len = len.min(file_size.saturating_sub(offset));
        if fcb.start_cluster == ClusterId::FREE || read_len == 0 {
            return Ok(Vec::new());
        }
        self.read_chain_bytes(fcb.start_cluster, offset, read_len)
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
        fcb.touch()?;
        self.write_fcb_at(open.loc, &fcb)?;
        debug!(
            "write(handle={}, loc={}, cursor={}, bytes={})",
            handle,
            open.loc,
            cursor,
            data.len()
        );

        let open_entry = self.open_files[slot].as_mut().expect("open slot");
        open_entry.cursor = new_end;
        open_entry.fcb = fcb;
        Ok(data.len())
    }

    pub fn write_file_at(
        &mut self,
        loc: DirEntryLoc,
        offset: usize,
        data: &[u8],
    ) -> Result<usize, FsError> {
        let fcb = self.read_fcb_at(loc)?;
        let _ = self.write_fcb_data_at(loc, fcb, offset, data)?;
        Ok(data.len())
    }

    pub fn set_mtime(&mut self, loc: DirEntryLoc, mtime: DateTime<Utc>) -> Result<(), FsError> {
        let mut fcb = self.read_fcb_at(loc)?;
        fcb.set_mdatetime(mtime)?;
        self.write_fcb_at(loc, &fcb)?;
        for open in self.open_files.iter_mut().flatten() {
            if open.loc == loc {
                open.fcb = fcb;
            }
        }
        debug!("set_mtime(loc={}, mtime={})", loc, mtime);
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), FsError> {
        if !self.fat_dirty {
            trace!("sync(), not dirty");
            return Ok(());
        }
        debug!("sync(), dirty");
        for copy_idx in 0..self.boot.fat_copies {
            self.flush_fat(copy_idx)?;
        }
        self.fat_dirty = false;
        Ok(())
    }

    pub fn dump_fat(&self) -> String {
        let mut out = String::new();
        for i in u16::from(ROOT_DIR_START_CLUSTER)..=u16::from(self.max_cluster_id()) {
            let value = self.read_fat(i.into()).unwrap_or(FatEntry::Free);
            out.push_str(&format!("{i}\t->\t{value}\n"));
        }
        out
    }

    fn write_boot_sector(&mut self) -> Result<(), FsError> {
        let mut block = vec![0; usize::from(self.boot.block_size)];
        block[..BOOT_SECTOR_SIZE].copy_from_slice(self.boot.as_bytes());
        self.write_device_block(BlockId(0), &block)?;
        trace!(
            "write_boot_sector(block_size={}, block_count={})",
            self.boot.block_size, self.boot.block_count
        );
        Ok(())
    }

    fn init_fat(&mut self) -> Result<(), FsError> {
        trace!(
            "init_fat(fat_copies={}, fat_block_count={})",
            self.boot.fat_copies, self.boot.fat_block_count
        );
        for copy in 0..self.boot.fat_copies {
            let start = self.fat_start_block_of(copy);
            for block in 0..self.boot.fat_block_count {
                self.zero_device_block(BlockId::from(u16::from(start) + block))?;
            }
        }
        Ok(())
    }

    fn init_root_dir(&mut self) -> Result<(), FsError> {
        self.write_fat(self.boot.root_dir_start_cluster, FatEntry::EndOfChain)?;
        self.zero_cluster(self.boot.root_dir_start_cluster)?;
        trace!(
            "init_root_dir(root_dir_start_cluster={})",
            self.boot.root_dir_start_cluster
        );
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
        let mut count = 0u32;
        for slot in self.raw_dir_slots(dir_start)? {
            if matches!(slot?.1, DirSlot::Occupied(_)) {
                count += 1;
            }
        }
        Ok(count * Fcb::SIZE as u32)
    }

    fn ensure_fcb_capacity(
        &mut self,
        mut fcb: Fcb,
        needed_clusters: usize,
    ) -> Result<Fcb, FsError> {
        let current = if fcb.start_cluster == ClusterId::FREE {
            0
        } else {
            self.chain_len_of(fcb.start_cluster)?
        };
        if needed_clusters <= current {
            return Ok(fcb);
        }
        let extra = self.allocate_clusters(needed_clusters - current)?;
        debug!(
            "ensure_fcb_capacity(start_cluster={}, current_clusters={}, needed_clusters={}), added_clusters={}",
            fcb.start_cluster,
            current,
            needed_clusters,
            extra.len()
        );
        if current == 0 {
            fcb.start_cluster = extra[0];
        } else {
            let last = self.chain_tail_of(fcb.start_cluster)?;
            self.write_fat(last, FatEntry::Next(extra[0]))?;
        }
        for (idx, cluster) in extra.iter().enumerate() {
            let next = extra
                .get(idx + 1)
                .copied()
                .map(FatEntry::Next)
                .unwrap_or(FatEntry::EndOfChain);
            self.write_fat(*cluster, next)?;
        }
        Ok(fcb)
    }

    fn find_open_slot(&self, handle: FileHandle) -> Result<usize, FsError> {
        self.open_files
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|open| open.handle == handle))
            .ok_or(FsError::InvalidHandle(handle))
    }

    fn node_directory_cluster(&self, node_id: NodeId) -> Result<ClusterId, FsError> {
        match node_id.into() {
            DirEntryLocFromNodeIdResult::Root => Ok(self.root_dir_cluster()),
            DirEntryLocFromNodeIdResult::Leaf(loc) => {
                let fcb = self.read_fcb_at(loc)?;
                if fcb.kind()? != NodeKind::Directory {
                    return Err(FsError::NotADirectory(fcb.short_name()));
                }
                Ok(fcb.start_cluster)
            }
        }
    }

    fn node_meta_from_fcb(&self, loc: DirEntryLoc, fcb: Fcb) -> Result<NodeMeta, FsError> {
        let kind = fcb.kind()?;
        Ok(NodeMeta {
            node_id: loc.into(),
            loc: Some(loc),
            short_name: fcb.short_name(),
            kind,
            size: self.size_of(&fcb)?,
            start_cluster: fcb.start_cluster,
            mtime: fcb.mtime,
            mdate: fcb.mdate,
        })
    }

    fn write_fcb_data_at(
        &mut self,
        loc: DirEntryLoc,
        mut fcb: Fcb,
        offset: usize,
        data: &[u8],
    ) -> Result<Fcb, FsError> {
        if fcb.kind()? == NodeKind::Directory {
            return Err(FsError::IsADirectory(fcb.short_name()));
        }
        let new_end = offset
            .checked_add(data.len())
            .ok_or(FsError::SeekOutOfBounds(offset))?;
        let needed_clusters = if new_end == 0 {
            0
        } else {
            new_end.div_ceil(self.cluster_size())
        };

        fcb = self.ensure_fcb_capacity(fcb, needed_clusters)?;
        if !data.is_empty() {
            self.write_chain_bytes(fcb.start_cluster, offset, data)?;
        }
        if new_end > usize::try_from(fcb.size).expect("file size must fit into usize") {
            fcb.size = u32::try_from(new_end).expect("file size exceeds u32 range");
        }
        fcb.touch()?;
        self.write_fcb_at(loc, &fcb)?;
        for open in self.open_files.iter_mut().flatten() {
            if open.loc == loc {
                open.fcb = fcb;
            }
        }
        Ok(fcb)
    }

    fn read_device_block(&self, block: BlockId) -> Result<Vec<u8>, FsError> {
        let mut out = vec![0; usize::from(self.boot.block_size)];
        self.device.borrow_mut().read_block_into(block, &mut out)?;
        Ok(out)
    }

    fn write_device_block(&self, block: BlockId, data: &[u8]) -> Result<(), FsError> {
        self.device.borrow_mut().write_block_from(block, data)
    }

    fn zero_device_block(&self, block: BlockId) -> Result<(), FsError> {
        self.device.borrow_mut().zero_block(block)
    }

    fn zero_cluster(&mut self, cluster: ClusterId) -> Result<(), FsError> {
        for block in self.cluster_blocks(cluster)? {
            self.zero_device_block(block)?;
        }
        Ok(())
    }

    /// Cluster size in bytes.
    fn cluster_size(&self) -> usize {
        usize::from(self.boot.block_size) * usize::from(self.boot.blocks_per_cluster)
    }

    fn fat_start_block_of(&self, copy_idx: u16) -> BlockId {
        BlockId::from(u16::from(self.boot.fat_start_block) + copy_idx * self.boot.fat_block_count)
    }

    fn cluster_first_block(&self, cluster: ClusterId) -> Result<BlockId, FsError> {
        if cluster < ROOT_DIR_START_CLUSTER || cluster > self.max_cluster_id() {
            return Err(FsError::CorruptFs(format!(
                "cluster {} outside data region",
                cluster
            )));
        }
        let first = u16::from(self.boot.data_start_block)
            + (u16::from(cluster) - u16::from(ROOT_DIR_START_CLUSTER))
                * self.boot.blocks_per_cluster;
        Ok(BlockId::from(first))
    }

    fn data_cluster_count(&self) -> u16 {
        (self.boot.block_count - u16::from(self.boot.data_start_block))
            / self.boot.blocks_per_cluster
    }

    fn max_cluster_id(&self) -> ClusterId {
        ClusterId::from(u16::from(ROOT_DIR_START_CLUSTER) + self.data_cluster_count() - 1)
    }

    fn cluster_blocks(&self, cluster: ClusterId) -> Result<Vec<BlockId>, FsError> {
        let first = self.cluster_first_block(cluster)?;
        Ok((0..self.boot.blocks_per_cluster)
            .map(|offset| BlockId::from(u16::from(first) + offset))
            .collect())
    }

    /// Get FAT block position of one cluster entry.
    fn fat_pos_of(&self, cluster: ClusterId) -> Result<(usize, usize), FsError> {
        if cluster < ROOT_DIR_START_CLUSTER || cluster > self.max_cluster_id() {
            return Err(FsError::CorruptFs(format!(
                "cluster {} outside data region",
                cluster
            )));
        }
        let offset = fat_offset(cluster);
        let block_offset = offset / usize::from(self.boot.block_size);
        let byte_offset = offset % usize::from(self.boot.block_size);
        trace!(
            "fat_pos_of(cluster={}), block_offset={}, byte_offset={}",
            cluster, block_offset, byte_offset
        );
        Ok((block_offset, byte_offset))
    }

    fn read_fat(&self, cluster: ClusterId) -> Result<FatEntry, FsError> {
        trace!("read_fat(cluster={})", cluster);
        // Assert position sanity
        let _ = self.fat_pos_of(cluster)?;
        if self.fat_m.len() != fat_entry_count(self.boot.block_size, self.boot.fat_block_count) {
            return Err(FsError::CorruptFs(
                "fat cache size does not match geometry".to_string(),
            ));
        }
        self.fat_m
            .get(usize::from(u16::from(cluster)))
            .copied()
            .ok_or_else(|| FsError::CorruptFs(format!("missing FAT cache entry for {}", cluster)))
    }

    fn write_fat(&mut self, cluster: ClusterId, value: FatEntry) -> Result<(), FsError> {
        trace!("write_fat(cluster={}, value={})", cluster, value);
        // Assert position sanity
        let _ = self.fat_pos_of(cluster)?;
        self.fat_m[usize::from(u16::from(cluster))] = value;
        self.fat_dirty = true;
        Ok(())
    }

    fn flush_fat(&mut self, copy_idx: u16) -> Result<(), FsError> {
        trace!("flush_fat(copy_idx={})", copy_idx);
        let mut bytes =
            vec![0; usize::from(self.boot.fat_block_count) * usize::from(self.boot.block_size)];
        for (index, entry) in self.fat_m.iter().copied().enumerate() {
            let start = index * FatEntry::SIZE;
            let end = start + FatEntry::SIZE;
            if end > bytes.len() {
                return Err(FsError::CorruptFs(
                    "fat cache larger than on-disk FAT region".to_string(),
                ));
            }
            bytes[start..end].copy_from_slice(&u16::from(entry).to_le_bytes());
        }
        let start_block = self.fat_start_block_of(copy_idx);
        for block_offset in 0..usize::from(self.boot.fat_block_count) {
            let start = block_offset * usize::from(self.boot.block_size);
            let end = start + usize::from(self.boot.block_size);
            self.write_device_block(
                BlockId::from(u16::from(start_block) + u16::try_from(block_offset).unwrap()),
                &bytes[start..end],
            )?;
        }
        Ok(())
    }

    fn chain_len_of(&self, start: ClusterId) -> Result<usize, FsError> {
        let mut len = 0;
        for cluster in ChainIter::new(self, start)? {
            cluster?;
            len += 1;
        }
        Ok(len)
    }

    fn chain_tail_of(&self, start: ClusterId) -> Result<ClusterId, FsError> {
        let mut last = None;
        for cluster in ChainIter::new(self, start)? {
            last = Some(cluster?);
        }
        last.ok_or(FsError::CorruptFs(format!(
            "chain head cluster {} invalid",
            start
        )))
    }

    fn try_for_each_cluster_mut<F>(&mut self, start: ClusterId, mut f: F) -> Result<(), FsError>
    where
        F: FnMut(&mut Self, ClusterId) -> Result<(), FsError>,
    {
        trace!("try_for_each_cluster_mut(start={})", start);
        if start == ClusterId::FREE {
            return Ok(());
        }
        let _ = self.fat_pos_of(start)?;
        let mut visited = HashSet::new();
        let mut current = start;
        loop {
            trace!("mutable chain walk. start={}, current={}", start, current);
            if !visited.insert(current) {
                return Err(FsError::CorruptFs(format!(
                    "cluster loop detected at {}",
                    current
                )));
            }
            let next = self.read_fat(current)?;
            match next {
                FatEntry::Free => {
                    return Err(FsError::CorruptFs(format!(
                        "cluster chain from {} reaches free entry",
                        start
                    )));
                }
                FatEntry::EndOfChain => {
                    trace!(
                        "mutable chain walk reached end. start={}, current={}",
                        start, current
                    );
                    f(self, current)?;
                    break;
                }
                FatEntry::Next(next_cluster) => {
                    trace!(
                        "mutable chain walk advance. current={}, next={}",
                        current, next_cluster
                    );
                    f(self, current)?;
                    current = next_cluster;
                }
            }
        }
        Ok(())
    }

    fn allocate_clusters(&mut self, len: usize) -> Result<Vec<ClusterId>, FsError> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            let cluster = (u16::from(ROOT_DIR_START_CLUSTER)..=u16::from(self.max_cluster_id()))
                .map(ClusterId::from)
                .find(|cluster| self.read_fat(*cluster).ok() == Some(FatEntry::Free))
                .ok_or(FsError::NoSpace)?;
            self.write_fat(cluster, FatEntry::EndOfChain)?;
            self.zero_cluster(cluster)?;
            out.push(cluster);
        }
        debug!("allocate_clusters(len={}), allocated={:?}", len, out);
        Ok(out)
    }

    fn free_chain_from(&mut self, start: ClusterId) -> Result<(), FsError> {
        debug!("free_chain_from(start={})", start);
        if start == ClusterId::FREE {
            return Ok(());
        }
        self.try_for_each_cluster_mut(start, |fs, cluster| {
            fs.write_fat(cluster, FatEntry::Free)?;
            fs.zero_cluster(cluster)?;
            Ok(())
        })?;
        Ok(())
    }

    fn advance_chain_iter_to(
        &self,
        start: ClusterId,
        cluster_index: usize,
    ) -> Result<ChainIter<'_, D>, FsError> {
        trace!(
            "advance_chain_iter_to(start={}, cluster_index={})",
            start, cluster_index
        );
        let mut iter = ChainIter::new(self, start)?;
        for step in 0..cluster_index {
            trace!(
                "advance chain iterator step. start={}, step={}, target_index={}",
                start, step, cluster_index
            );
            iter.next()
                .transpose()?
                .ok_or_else(|| FsError::CorruptFs("offset beyond cluster chain".to_string()))?;
        }
        Ok(iter)
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
        trace!(
            "read_chain_bytes(start={}, offset={}, len={})",
            start, offset, len
        );
        let cluster_size = self.cluster_size();
        let mut out = Vec::with_capacity(len);
        let mut remaining = len;
        let mut cursor = offset;
        let mut iter = self.advance_chain_iter_to(start, cursor / cluster_size)?;
        while remaining > 0 {
            let cluster = iter
                .next()
                .transpose()?
                .ok_or_else(|| FsError::CorruptFs("offset beyond cluster chain".to_string()))?;
            let cluster_bytes = self.read_cluster_bytes(cluster)?;
            let in_cluster = cursor % cluster_size;
            let chunk = remaining.min(cluster_size - in_cluster);
            trace!(
                "read chain chunk. cluster={}, in_cluster={}, chunk={}",
                cluster, in_cluster, chunk
            );
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
        trace!(
            "write_chain_bytes(start={}, offset={}, len={})",
            start,
            offset,
            data.len()
        );
        let cluster_size = self.cluster_size();
        let mut remaining = data.len();
        let mut cursor = offset;
        let mut written = 0;
        let mut visited = HashSet::new();
        let mut current = start;
        for _ in 0..(cursor / cluster_size) {
            if !visited.insert(current) {
                return Err(FsError::CorruptFs(format!(
                    "cluster loop detected at {}",
                    current
                )));
            }
            current = match self.read_fat(current)? {
                FatEntry::Next(next) => next,
                FatEntry::Free => {
                    return Err(FsError::CorruptFs(format!(
                        "cluster chain from {} reaches free entry",
                        start
                    )));
                }
                FatEntry::EndOfChain => {
                    return Err(FsError::CorruptFs(
                        "offset beyond cluster chain".to_string(),
                    ));
                }
            };
        }
        while remaining > 0 {
            if !visited.insert(current) {
                return Err(FsError::CorruptFs(format!(
                    "cluster loop detected at {}",
                    current
                )));
            }
            let next = self.read_fat(current)?;
            let mut cluster_bytes = self.read_cluster_bytes(current)?;
            let in_cluster = cursor % cluster_size;
            let chunk = remaining.min(cluster_size - in_cluster);
            trace!(
                "write chain chunk. cluster={}, in_cluster={}, chunk={}",
                current, in_cluster, chunk
            );
            cluster_bytes[in_cluster..in_cluster + chunk]
                .copy_from_slice(&data[written..written + chunk]);
            self.write_cluster_bytes(current, &cluster_bytes)?;
            cursor += chunk;
            written += chunk;
            remaining -= chunk;
            if remaining > 0 {
                current = match next {
                    FatEntry::Next(next) => next,
                    FatEntry::Free => {
                        return Err(FsError::CorruptFs(format!(
                            "cluster chain from {} reaches free entry",
                            start
                        )));
                    }
                    FatEntry::EndOfChain => {
                        return Err(FsError::CorruptFs(
                            "offset beyond cluster chain".to_string(),
                        ));
                    }
                };
            }
        }
        Ok(())
    }

    fn read_cluster_bytes(&self, cluster: ClusterId) -> Result<Vec<u8>, FsError> {
        let mut out = Vec::with_capacity(self.cluster_size());
        for block in self.cluster_blocks(cluster)? {
            out.extend_from_slice(&self.read_device_block(block)?);
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
            self.write_device_block(block, &data[start..end])?;
        }
        Ok(())
    }

    fn raw_dir_slots(&self, dir_start: ClusterId) -> Result<RawDirSlotIter<'_, D>, FsError> {
        RawDirSlotIter::new(self, dir_start)
    }

    fn read_fcb_at(&self, loc: DirEntryLoc) -> Result<Fcb, FsError> {
        match self.read_slot(loc)? {
            DirSlot::Occupied(fcb) => Ok(fcb),
            _ => Err(FsError::NotFoundAt(loc)),
        }
    }

    fn read_slot(&self, loc: DirEntryLoc) -> Result<DirSlot, FsError> {
        trace!(
            "read_slot(dir_start={}, entry_index={})",
            loc.dir_start, loc.entry_index
        );
        let bytes = self.read_chain_bytes(loc.dir_start, self.slot_offset(loc), Fcb::SIZE)?;
        DirSlot::try_from(bytes.as_slice())
    }

    fn write_fcb_at(&mut self, loc: DirEntryLoc, fcb: &Fcb) -> Result<(), FsError> {
        trace!(
            "write_fcb_at(dir_start={}, entry_index={}), short_name={}",
            loc.dir_start,
            loc.entry_index,
            fcb.short_name()
        );
        let mut bytes = [0; Fcb::SIZE];
        fcb.write_to_slice(&mut bytes)?;
        self.write_chain_bytes(loc.dir_start, self.slot_offset(loc), &bytes)
    }

    fn mark_slot_deleted(&mut self, loc: DirEntryLoc) -> Result<(), FsError> {
        trace!(
            "mark_slot_deleted(dir_start={}, entry_index={})",
            loc.dir_start, loc.entry_index
        );
        let mut bytes = [0; Fcb::SIZE];
        bytes[0] = DirSlot::SLOT_DELETED;
        self.write_chain_bytes(loc.dir_start, self.slot_offset(loc), &bytes)
    }

    fn slot_offset(&self, loc: DirEntryLoc) -> usize {
        usize::try_from(loc.entry_index).unwrap() * Fcb::SIZE
    }

    fn find_free_dir_slot(&mut self, dir_start: ClusterId) -> Result<DirEntryLoc, FsError> {
        let mut next_entry_index = 0u32;
        trace!("find_free_dir_slot(dir_start={})", dir_start);
        for slot in self.raw_dir_slots(dir_start)? {
            let (loc, slot) = slot?;
            next_entry_index = loc.entry_index + 1;
            match slot {
                DirSlot::Unused | DirSlot::Deleted => {
                    trace!(
                        "found free dir slot. dir_start={}, entry_index={}",
                        dir_start, loc.entry_index
                    );
                    return Ok(loc);
                }
                DirSlot::Occupied(_) => {}
            }
        }

        let new_cluster = self.allocate_clusters(1)?[0];
        let last = self.chain_tail_of(dir_start)?;
        debug!(
            "extend directory chain. dir_start={}, last_cluster={}, new_cluster={}",
            dir_start, last, new_cluster
        );
        self.write_fat(last, FatEntry::Next(new_cluster))?;
        self.write_fat(new_cluster, FatEntry::EndOfChain)?;
        Ok(DirEntryLoc {
            dir_start,
            entry_index: next_entry_index,
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
            fcb.touch()?;
            self.write_fcb_at(loc, &fcb)?;
        }
        Ok(())
    }

    fn find_dir_loc(
        &self,
        dir_start: ClusterId,
        target: ClusterId,
    ) -> Result<Option<DirEntryLoc>, FsError> {
        for slot in self.raw_dir_slots(dir_start)? {
            let (loc, slot) = slot?;
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
        let data_start = 1u32 + u32::from(fat_copies) * u32::from(fat_blocks);
        if data_start >= u32::from(block_count) {
            return fat_blocks;
        }
        let data_clusters = (u32::from(block_count) - data_start) / u32::from(blocks_per_cluster);
        let fat_entries = usize::from(u16::from(ROOT_DIR_START_CLUSTER))
            + usize::try_from(data_clusters).unwrap();
        let fat_bytes = fat_entries * 2;
        let needed = fat_bytes.div_ceil(usize::from(block_size)) as u16;
        if needed <= fat_blocks {
            return fat_blocks;
        }
        fat_blocks = needed;
    }
}

fn fat_entry_count(block_size: u16, fat_block_count: u16) -> usize {
    usize::from(block_size) * usize::from(fat_block_count) / FatEntry::SIZE
}

fn read_boot_sector_from_device<D: BufferedBlockDevice>(
    device: &mut D,
) -> Result<BootSector, FsError> {
    let mut block = vec![0; device.block_size()];
    device.read_block_into(BlockId(0), &mut block)?;
    BootSector::read_from_prefix(&block)
}

fn validate_boot_sector_against_device<D: BufferedBlockDevice>(
    boot: &BootSector,
    device: &D,
) -> Result<(), FsError> {
    if usize::from(boot.block_size) != device.block_size() {
        return Err(FsError::CorruptFs(format!(
            "boot sector block size {} does not match device block size {}",
            boot.block_size,
            device.block_size()
        )));
    }
    if usize::from(boot.block_count) > device.block_count() {
        return Err(FsError::CorruptFs(format!(
            "boot sector block count {} exceeds device block count {}",
            boot.block_count,
            device.block_count()
        )));
    }
    FsConfig {
        block_size: boot.block_size,
        block_count: boot.block_count,
        blocks_per_cluster: boot.blocks_per_cluster,
    }
    .validate()?;

    let expected_fat_block_count = get_fat_block_count(
        boot.block_size,
        boot.block_count,
        boot.fat_copies,
        boot.blocks_per_cluster,
    );
    if boot.fat_block_count != expected_fat_block_count {
        return Err(FsError::CorruptFs(format!(
            "boot sector fat block count {} does not match computed value {}",
            boot.fat_block_count, expected_fat_block_count
        )));
    }
    if boot.fat_start_block != BlockId(1) {
        return Err(FsError::CorruptFs(format!(
            "unexpected fat start block {}",
            boot.fat_start_block
        )));
    }
    if boot.data_start_block != BlockId(1 + boot.fat_copies * boot.fat_block_count) {
        return Err(FsError::CorruptFs(format!(
            "unexpected data start block {}",
            boot.data_start_block
        )));
    }
    if boot.root_dir_start_cluster != ROOT_DIR_START_CLUSTER {
        return Err(FsError::CorruptFs(format!(
            "unexpected root dir start cluster {}",
            boot.root_dir_start_cluster
        )));
    }
    Ok(())
}

fn read_fat_cache_from_device<D: BufferedBlockDevice>(
    device: &mut D,
    boot: &BootSector,
) -> Result<Vec<FatEntry>, FsError> {
    let mut fat_bytes = vec![0; usize::from(boot.block_size) * usize::from(boot.fat_block_count)];
    for block_offset in 0..usize::from(boot.fat_block_count) {
        let start = block_offset * usize::from(boot.block_size);
        let end = start + usize::from(boot.block_size);
        device.read_block_into(
            BlockId::from(u16::from(boot.fat_start_block) + u16::try_from(block_offset).unwrap()),
            &mut fat_bytes[start..end],
        )?;
    }

    let mut fat_m = Vec::with_capacity(fat_entry_count(boot.block_size, boot.fat_block_count));
    for chunk in fat_bytes.chunks_exact(FatEntry::SIZE) {
        let raw = u16::from_le_bytes([chunk[0], chunk[1]]);
        fat_m.push(FatEntry::from(raw));
    }
    Ok(fat_m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use tempfile::tempdir;

    fn mkmemfs() -> MyFileSystem<MemoryBlockDevice> {
        MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig::default())
            .expect("filesystem should format")
    }

    fn mkfiledev(
        path: &std::path::Path,
        block_size: usize,
        block_count: usize,
    ) -> LogicalBlockDevice<FileBlockDevice> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        LogicalBlockDevice::new(
            FileBlockDevice::create(file, block_size, block_count).unwrap(),
            block_size,
        )
        .unwrap()
    }

    fn read_fat_copy_bytes(fs: &MyFileSystem<MemoryBlockDevice>, copy: usize) -> Vec<u8> {
        let fat_start = usize::from(u16::from(fs.boot.fat_start_block));
        let fat_blocks = usize::from(fs.boot.fat_block_count);
        let mut out = Vec::with_capacity(fat_blocks * usize::from(fs.boot.block_size));
        for block in 0..fat_blocks {
            let block_id =
                BlockId::from(u16::try_from(fat_start + copy * fat_blocks + block).unwrap());
            out.extend_from_slice(&fs.read_device_block(block_id).unwrap());
        }
        out
    }

    fn collect_dir_entries(
        fs: &MyFileSystem<MemoryBlockDevice>,
        dir_start: ClusterId,
    ) -> Vec<DirEntry> {
        fs.dir_entries(dir_start)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn parse_dir_slot_recognizes_unused_and_deleted() {
        let unused = [0u8; Fcb::SIZE];
        assert!(matches!(
            DirSlot::try_from(unused.as_slice()).unwrap(),
            DirSlot::Unused
        ));

        let mut deleted = [0u8; Fcb::SIZE];
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
                block_size: 8,
                block_count: 128,
                blocks_per_cluster: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 128,
                block_count: 2,
                blocks_per_cluster: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 16,
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
                block_size: 1024,
                block_count: 128,
                blocks_per_cluster: 3,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn format_writes_boot_and_two_fat_copies() {
        let fs = mkmemfs();
        let boot_block = fs.read_device_block(BlockId(0)).unwrap();
        assert_eq!(
            u16::from_le_bytes([boot_block[0], boot_block[1]]),
            DEFAULT_BLOCK_SIZE as u16
        );

        let fat1 = fs.read_device_block(BlockId(1)).unwrap();
        let fat2 = fs.read_device_block(BlockId(2)).unwrap();
        assert_eq!(fat1, fat2);
        assert_eq!(
            fs.read_fat(ROOT_DIR_START_CLUSTER).unwrap(),
            FatEntry::EndOfChain
        );
        assert_eq!(collect_dir_entries(&fs, fs.root_dir_cluster()).len(), 0);
    }

    #[test]
    fn open_on_device_reads_formatted_image() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("open-on-device.img");
        let config = FsConfig {
            block_size: 128,
            block_count: 256,
            blocks_per_cluster: 2,
        };

        let mut fs = MyFileSystem::format_on_device(
            mkfiledev(
                &path,
                usize::from(config.block_size),
                usize::from(config.block_count),
            ),
            config.clone(),
        )
        .unwrap();
        fs.create_file(fs.root_dir_cluster(), "HELLO.TXT").unwrap();
        fs.sync().unwrap();
        drop(fs);

        let reopened = MyFileSystem::open_on_device(
            LogicalBlockDevice::new(
                FileBlockDevice::from_file(
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .unwrap(),
                    usize::from(config.block_size),
                )
                .unwrap(),
                usize::from(config.block_size),
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(reopened.boot_sector().block_size, config.block_size);
        assert_eq!(reopened.boot_sector().block_count, config.block_count);
        assert!(
            reopened
                .lookup(reopened.root_dir_cluster(), "HELLO.TXT")
                .is_ok()
        );
        assert_eq!(
            reopened.read_fat(ROOT_DIR_START_CLUSTER).unwrap(),
            FatEntry::EndOfChain
        );
    }

    #[test]
    fn file_backed_round_trip_persists_file_data() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("file-backed-round-trip.img");
        let config = FsConfig::default();

        let mut fs = MyFileSystem::format_on_device(
            mkfiledev(
                &path,
                usize::from(config.block_size),
                usize::from(config.block_count),
            ),
            config.clone(),
        )
        .unwrap();
        let loc = fs.create_file(fs.root_dir_cluster(), "NOTE.TXT").unwrap();
        fs.write_file_at(loc, 0, b"abc123").unwrap();
        fs.sync().unwrap();
        drop(fs);

        let reopened = MyFileSystem::open_on_device(
            LogicalBlockDevice::new(
                FileBlockDevice::from_file(
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .unwrap(),
                    usize::from(config.block_size),
                )
                .unwrap(),
                usize::from(config.block_size),
            )
            .unwrap(),
        )
        .unwrap();
        let (loc, _) = reopened
            .lookup(reopened.root_dir_cluster(), "NOTE.TXT")
            .unwrap();
        let data = reopened.read_file_at(loc, 0, 64).unwrap();
        assert_eq!(data, b"abc123");
    }

    #[test]
    fn open_on_device_accepts_backing_file_larger_than_filesystem_image() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("oversized-backing.img");
        let config = FsConfig {
            block_size: 128,
            block_count: 64,
            blocks_per_cluster: 2,
        };

        let mut fs =
            MyFileSystem::format_on_device(mkfiledev(&path, 128, 96), config.clone()).unwrap();
        fs.create_file(fs.root_dir_cluster(), "BIGFILE.TXT")
            .unwrap();
        fs.sync().unwrap();
        drop(fs);

        let reopened = MyFileSystem::open_on_device(
            LogicalBlockDevice::new(
                FileBlockDevice::from_file(
                    OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .unwrap(),
                    usize::from(config.block_size),
                )
                .unwrap(),
                usize::from(config.block_size),
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(reopened.boot_sector().block_count, config.block_count);
        assert!(
            reopened
                .lookup(reopened.root_dir_cluster(), "BIGFILE.TXT")
                .is_ok()
        );
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
    fn format_accepts_large_non_default_block_size() {
        let fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 2048,
            block_count: 128,
            blocks_per_cluster: 2,
        })
        .unwrap();
        assert_eq!(fs.boot.block_size, 2048);
        assert_eq!(usize::from(fs.boot.block_size), 2048);
        assert_eq!(fs.cluster_size(), 4096);
    }

    #[test]
    fn format_rejects_cluster_smaller_than_one_fcb() {
        let result = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 16,
            block_count: 128,
            blocks_per_cluster: 1,
        });
        assert!(matches!(result, Err(FsError::InvalidConfig(_))));
    }

    #[test]
    fn root_directory_chain_grows_like_normal_directory() {
        let mut fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 64,
            block_count: 256,
            blocks_per_cluster: 1,
        })
        .unwrap();
        for idx in 0..4 {
            fs.create_file(fs.root_dir_cluster(), &format!("R{idx}.TXT"))
                .unwrap();
        }
        let chain = ChainIter::new(&fs, fs.root_dir_cluster())
            .unwrap()
            .collect::<Result<Vec<ClusterId>, FsError>>()
            .unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0], ROOT_DIR_START_CLUSTER);
    }

    #[test]
    fn fat_block_count_computation_does_not_oscillate() {
        assert_eq!(get_fat_block_count(128, 256, 2, 4), 2);
    }

    #[test]
    fn node_oriented_api_resolves_and_stats() {
        let mut fs = mkmemfs();
        let docs_loc = fs.mkdir(fs.root_dir_cluster(), "DOCS").unwrap();
        let docs_node = fs.lookup_node(fs.root_node(), "DOCS").unwrap();
        let docs_meta = fs.stat_node(docs_node).unwrap();
        assert_eq!(docs_meta.loc, Some(docs_loc));
        assert_eq!(docs_meta.short_name, "DOCS");
        assert_eq!(docs_meta.kind, NodeKind::Directory);

        let readme_loc = fs
            .create_file(docs_meta.start_cluster, "README.TXT")
            .unwrap();
        let readme_node = fs.lookup_node(docs_node, "README.TXT").unwrap();
        let readme_meta = fs.stat_node(readme_node).unwrap();
        assert_eq!(readme_meta.loc, Some(readme_loc));
        assert_eq!(readme_meta.short_name, "README.TXT");

        let entries = fs
            .dir_entries_node(docs_node)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].node_id, readme_node);

        let handle = fs.open_node(readme_node).unwrap();
        fs.close(handle).unwrap();
    }

    #[test]
    fn packed_fcb_slots_can_cross_block_boundaries() {
        let mut fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 64,
            block_count: 256,
            blocks_per_cluster: 2,
        })
        .unwrap();

        let a = fs.create_file(fs.root_dir_cluster(), "A.TXT").unwrap();
        let b = fs.create_file(fs.root_dir_cluster(), "B.TXT").unwrap();
        let c = fs.create_file(fs.root_dir_cluster(), "C.TXT").unwrap();
        let d = fs.create_file(fs.root_dir_cluster(), "D.TXT").unwrap();

        assert_eq!(a.entry_index, 0);
        assert_eq!(b.entry_index, 1);
        assert_eq!(c.entry_index, 2);
        assert_eq!(d.entry_index, 3);

        assert_eq!(fs.read_fcb_at(d).unwrap().short_name(), "D.TXT");
        let names = fs
            .dir_entries(fs.root_dir_cluster())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .map(|entry| entry.short_name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["A.TXT", "B.TXT", "C.TXT", "D.TXT"]);
    }

    #[test]
    fn io_works_with_multi_block_clusters() {
        let mut fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 128,
            block_count: 256,
            blocks_per_cluster: 4,
        })
        .unwrap();

        let file_loc = fs.create_file(fs.root_dir_cluster(), "DATA.BIN").unwrap();
        let handle = fs.open(file_loc).unwrap();
        let payload = vec![0xCD; fs.cluster_size() + 37];
        assert_eq!(fs.write(handle, &payload).unwrap(), payload.len());
        fs.seek(handle, 0).unwrap();
        assert_eq!(fs.read(handle, payload.len()).unwrap(), payload);
    }

    #[test]
    fn io_works_with_large_blocks_and_clusters() {
        let mut fs = MyFileSystem::<MemoryBlockDevice>::format_memory(FsConfig {
            block_size: 2048,
            block_count: 128,
            blocks_per_cluster: 8,
        })
        .unwrap();

        let file_loc = fs.create_file(fs.root_dir_cluster(), "BIG.BIN").unwrap();
        let handle = fs.open(file_loc).unwrap();
        let payload = vec![0x5A; fs.cluster_size() * 2 + 11];
        assert_eq!(fs.write(handle, &payload).unwrap(), payload.len());
        fs.seek(handle, 0).unwrap();
        assert_eq!(fs.read(handle, payload.len()).unwrap(), payload);
    }

    #[test]
    fn fat_copies_stay_in_sync_after_mutations() {
        let mut fs = mkmemfs();
        let file_loc = fs.create_file(fs.root_dir_cluster(), "SYNC.BIN").unwrap();
        let handle = fs.open(file_loc).unwrap();
        let payload = vec![0xEF; DEFAULT_BLOCK_SIZE + 99];
        fs.write(handle, &payload).unwrap();
        fs.close(handle).unwrap();
        fs.remove_file(file_loc).unwrap();
        fs.sync().unwrap();

        assert_eq!(read_fat_copy_bytes(&fs, 0), read_fat_copy_bytes(&fs, 1));
    }

    #[test]
    fn fat_cache_defers_disk_updates_until_sync() {
        let mut fs = mkmemfs();
        let fat1_before = read_fat_copy_bytes(&fs, 0);
        let fat2_before = read_fat_copy_bytes(&fs, 1);

        let file_loc = fs.create_file(fs.root_dir_cluster(), "CACHE.BIN").unwrap();
        let handle = fs.open(file_loc).unwrap();
        fs.write(handle, &[0xAA; 32]).unwrap();
        fs.close(handle).unwrap();

        assert!(fs.fat_dirty);
        assert_ne!(
            fs.read_fat(fs.read_fcb_at(file_loc).unwrap().start_cluster)
                .unwrap(),
            FatEntry::Free
        );
        assert_eq!(read_fat_copy_bytes(&fs, 0), fat1_before);
        assert_eq!(read_fat_copy_bytes(&fs, 1), fat2_before);

        fs.sync().unwrap();

        assert!(!fs.fat_dirty);
        assert_eq!(read_fat_copy_bytes(&fs, 0), read_fat_copy_bytes(&fs, 1));
        assert_ne!(read_fat_copy_bytes(&fs, 0), fat1_before);
    }

    #[test]
    fn creates_lists_and_removes_directories() {
        let mut fs = mkmemfs();
        let docs_loc = fs.mkdir(fs.root_dir_cluster(), "DOCS").unwrap();
        let docs_cluster = fs.read_fcb_at(docs_loc).unwrap().start_cluster;
        let readme_loc = fs.create_file(docs_cluster, "README.TXT").unwrap();

        let root = collect_dir_entries(&fs, fs.root_dir_cluster());
        assert_eq!(root.len(), 1);
        assert_eq!(root[0].short_name, "DOCS");
        assert_eq!(root[0].kind, NodeKind::Directory);
        assert_eq!(root[0].loc, docs_loc);

        let docs = collect_dir_entries(&fs, docs_cluster);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].short_name, "README.TXT");
        assert_eq!(docs[0].loc, readme_loc);

        assert!(matches!(
            fs.rmdir(docs_loc),
            Err(FsError::DirectoryNotEmpty(_))
        ));

        fs.remove_file(readme_loc).unwrap();
        fs.rmdir(docs_loc).unwrap();
        assert!(collect_dir_entries(&fs, fs.root_dir_cluster()).is_empty());
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
        assert_eq!(fs.chain_len_of(start).unwrap(), 2);
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
