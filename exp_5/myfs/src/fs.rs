use crate::util::*;

use std::fmt;

pub const DEFAULT_BLOCK_SIZE: usize = 1024;
pub const DEFAULT_BLOCK_COUNT: u16 = 128;
pub const ROOT_DIR_START_CLUSTER: ClusterId = ClusterId(2);
pub const ROOT_DIR_CLUSTER_COUNT: u16 = 2;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClusterId(pub u16);

impl fmt::Display for ClusterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ClusterId {
    pub const FAT_FREE: u16 = 0x0000;
    pub const FAT_EOC: u16 = 0xFFFF;

    pub const FREE: Self = Self(Self::FAT_FREE);
    pub const EOC: Self = Self(Self::FAT_EOC);
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u16);

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileHandle(pub u32);

impl fmt::Display for FileHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DirEntryLoc {
    pub dir_start: ClusterId,
    pub entry_index: u32,
}

impl fmt::Display for DirEntryLoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.dir_start.0, self.entry_index)
    }
}

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

impl std::error::Error for FsError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FcbAttr(pub u8);

impl FcbAttr {
    pub const DIR_ATTR: u8 = 0x10;
    pub const FILE_ATTR: u8 = 0x20;

    pub const FILE: Self = Self(Self::FILE_ATTR);
    pub const DIRECTORY: Self = Self(Self::DIR_ATTR);
}

impl TryFrom<FcbAttr> for NodeKind {
    type Error = FsError;

    fn try_from(value: FcbAttr) -> Result<Self, Self::Error> {
        match value {
            FcbAttr::FILE => Ok(NodeKind::File),
            FcbAttr::DIRECTORY => Ok(NodeKind::Directory),
            _ => Err(FsError::CorruptFs(format!(
                "unknown attr byte: {:#x}",
                value.0
            ))),
        }
    }
}

impl From<NodeKind> for FcbAttr {
    fn from(value: NodeKind) -> Self {
        match value {
            NodeKind::File => FcbAttr::FILE,
            NodeKind::Directory => FcbAttr::DIRECTORY,
        }
    }
}

impl NodeKind {
    pub fn attr(self) -> FcbAttr {
        self.into()
    }

    pub fn label(self) -> &'static str {
        match self {
            NodeKind::File => "FILE",
            NodeKind::Directory => "DIR",
        }
    }
}

/// On-disk boot-region metadata for filesystem image.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootSector {
    pub block_size: u16,
    pub block_count: u16,
    pub blocks_per_cluster: u16,
    pub fat_start_block: BlockId,
    pub fat_block_count: u16,
    pub fat_copies: u16,
    pub data_start_block: BlockId,
    pub root_dir_start_cluster: ClusterId,
    pub root_dir_cluster_count: u16,
}

/// In-memory formatter input.
#[derive(Debug, Clone)]
pub struct FsConfig {
    pub block_size: u16,
    pub block_count: u16,
}

impl Default for FsConfig {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE as u16,
            block_count: DEFAULT_BLOCK_COUNT,
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
        Ok(())
    }
}

/// On-disk file control block stored inside directory slots.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fcb {
    pub file_name: [u8; 8],
    pub ext_name: [u8; 3],
    pub attr: FcbAttr,
    pub ctime: u16,
    pub cdate: u16,
    pub start_cluster: ClusterId,
    pub size: u32,
}

impl Fcb {
    pub(crate) fn new(
        name: &str,
        kind: NodeKind,
        start_cluster: ClusterId,
        size: u32,
    ) -> Result<Self, FsError> {
        let encoded = encode_short_name(name)?;
        Ok(Self {
            file_name: encoded.0,
            ext_name: encoded.1,
            attr: kind.attr(),
            ctime: 0,
            cdate: 0,
            start_cluster,
            size,
        })
    }

    pub fn short_name(&self) -> String {
        let base = decode_name_part(&self.file_name);
        let ext = decode_name_part(&self.ext_name);
        if ext.is_empty() {
            base
        } else {
            format!("{base}.{ext}")
        }
    }

    pub fn kind(&self) -> Result<NodeKind, FsError> {
        self.attr.try_into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_and_attr_convert_both_ways() {
        assert_eq!(FcbAttr::from(NodeKind::File), FcbAttr::FILE);
        assert_eq!(FcbAttr::from(NodeKind::Directory), FcbAttr::DIRECTORY);
        assert_eq!(NodeKind::try_from(FcbAttr::FILE).unwrap(), NodeKind::File);
        assert_eq!(
            NodeKind::try_from(FcbAttr::DIRECTORY).unwrap(),
            NodeKind::Directory
        );
        assert!(NodeKind::try_from(FcbAttr(0x7F)).is_err());
    }

    #[test]
    fn fs_config_validation_rejects_bad_values() {
        assert!(
            FsConfig {
                block_size: 63,
                block_count: 128
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 128,
                block_count: 8
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 96,
                block_count: 128
            }
            .validate()
            .is_err()
        );
        assert!(
            FsConfig {
                block_size: 128,
                block_count: 128
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn fcb_new_keeps_requested_minimal_fields() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId::FREE, 0).unwrap();
        assert_eq!(fcb.ctime, 0);
        assert_eq!(fcb.cdate, 0);
        assert_eq!(fcb.attr, FcbAttr::FILE);
        assert_eq!(fcb.start_cluster, ClusterId::FREE);
        assert_eq!(fcb.size, 0);
        assert_eq!(fcb.short_name(), "A.TXT");
    }
}
