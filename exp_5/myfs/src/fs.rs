//! File system abstractions and operations.

use bytemuck::Pod;
use bytemuck::Zeroable;
use derive_more::Deref;
use derive_more::DerefMut;
use derive_more::Display;
use derive_more::From;
use derive_more::Into;
use std::fmt;

use crate::FsError;
use crate::ShortName;

pub const DEFAULT_BLOCK_SIZE: usize = 1024;
pub const DEFAULT_BLOCK_COUNT: u16 = 128;
pub const DEFAULT_BLOCKS_PER_CLUSTER: u16 = 1;
pub const MAX_BLOCK_SIZE: usize = 1024;
pub const ROOT_DIR_START_CLUSTER: ClusterId = ClusterId(2);
pub const ROOT_DIR_CLUSTER_COUNT: u16 = 2;

/// The ID of a FAT cluster.
/// TODO: Currently one cluster equals one block. Make it parametric!
#[repr(transparent)]
#[derive(
    Deref,
    DerefMut,
    Display,
    From,
    Into,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Zeroable,
    Pod,
)]
#[display("{_0}")]
pub struct ClusterId(pub u16);

impl ClusterId {
    pub const FAT_FREE: u16 = 0x0000;
    pub const FAT_EOC: u16 = 0xFFFF;

    pub const FREE: Self = Self(Self::FAT_FREE);
    pub const EOC: Self = Self(Self::FAT_EOC);
}

/// The ID of a block, aka sector.
#[repr(transparent)]
#[derive(
    Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq, Hash, Zeroable, Pod,
)]
#[display("{_0}")]
pub struct BlockId(pub u16);

#[repr(transparent)]
#[derive(
    Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq, Hash, Zeroable, Pod,
)]
#[display("{_0}")]
pub struct FileHandle(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeKind::File => write!(f, "FILE"),
            NodeKind::Directory => write!(f, "DIR"),
        }
    }
}

#[repr(transparent)]
#[derive(
    Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq, Zeroable, Pod,
)]
#[display("{_0}")]
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

/// On-disk boot-region metadata for filesystem image.
///
/// Stored at the beginning of reserved sector 0. This record is a prefix of the first block, not
/// a whole-block object.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Zeroable, Pod)]
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

pub const BOOT_SECTOR_SIZE: usize = std::mem::size_of::<BootSector>();
const _: () = assert!(BOOT_SECTOR_SIZE <= MAX_BLOCK_SIZE);

impl BootSector {
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }

    pub fn read_from_prefix(bytes: &[u8]) -> Result<Self, FsError> {
        let bytes = bytes
            .get(..BOOT_SECTOR_SIZE)
            .ok_or_else(|| FsError::CorruptFs("boot sector shorter than expected".to_string()))?;
        Ok(bytemuck::pod_read_unaligned(bytes))
    }
}

/// On-disk file control block stored inside directory slots.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Zeroable, Pod)]
pub struct Fcb {
    pub short_name: ShortName,
    pub attr: FcbAttr,
    pub reserved: u8,
    pub ctime: u16,
    pub cdate: u16,
    pub start_cluster: ClusterId,
    pub size: u32,
}

pub const FCB_SIZE: usize = std::mem::size_of::<Fcb>();

impl Fcb {
    pub(crate) fn new(
        name: &str,
        kind: NodeKind,
        start_cluster: ClusterId,
        size: u32,
    ) -> Result<Self, FsError> {
        Ok(Self {
            short_name: ShortName::try_from(name)?,
            attr: kind.into(),
            reserved: 0,
            ctime: 0,
            cdate: 0,
            start_cluster,
            size,
        })
    }

    pub fn short_name(&self) -> String {
        self.short_name.to_string()
    }

    pub fn kind(&self) -> Result<NodeKind, FsError> {
        self.attr.try_into()
    }

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }

    pub fn write_to_slice(&self, dst: &mut [u8]) -> Result<(), FsError> {
        let dst = dst
            .get_mut(..FCB_SIZE)
            .ok_or_else(|| FsError::CorruptFs("fcb slot shorter than expected".to_string()))?;
        dst.copy_from_slice(self.as_bytes());
        Ok(())
    }

    pub fn read_from_bytes(bytes: &[u8]) -> Result<Self, FsError> {
        let bytes = bytes
            .get(..FCB_SIZE)
            .ok_or_else(|| FsError::CorruptFs("fcb slot shorter than expected".to_string()))?;
        Ok(bytemuck::pod_read_unaligned(bytes))
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
    fn fcb_new_keeps_requested_minimal_fields() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId::FREE, 0).unwrap();
        assert_eq!(fcb.ctime, 0);
        assert_eq!(fcb.cdate, 0);
        assert_eq!(fcb.attr, FcbAttr::FILE);
        assert_eq!(fcb.start_cluster, ClusterId::FREE);
        assert_eq!(fcb.size, 0);
        assert_eq!(fcb.short_name(), "A.TXT");
    }

    #[test]
    fn boot_sector_bytes_round_trip() {
        let boot = BootSector {
            block_size: 1024,
            block_count: 128,
            blocks_per_cluster: 1,
            fat_start_block: BlockId(1),
            fat_block_count: 1,
            fat_copies: 2,
            data_start_block: BlockId(3),
            root_dir_start_cluster: ROOT_DIR_START_CLUSTER,
            root_dir_cluster_count: ROOT_DIR_CLUSTER_COUNT,
        };
        assert_eq!(boot.as_bytes().len(), BOOT_SECTOR_SIZE);
        assert_eq!(BootSector::read_from_prefix(boot.as_bytes()).unwrap(), boot);
    }

    #[test]
    fn fcb_bytes_round_trip() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId(7), 123).unwrap();
        assert_eq!(fcb.as_bytes().len(), FCB_SIZE);
        assert_eq!(Fcb::read_from_bytes(fcb.as_bytes()).unwrap(), fcb);
    }

    #[test]
    fn write_to_slice_rejects_short_buffer() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId::FREE, 0).unwrap();
        let mut short = [0u8; FCB_SIZE - 1];
        assert!(fcb.write_to_slice(&mut short).is_err());
    }
}
