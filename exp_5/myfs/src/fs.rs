//! File system abstractions and operations.

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
pub const ROOT_DIR_START_CLUSTER: ClusterId = ClusterId(2);
pub const ROOT_DIR_CLUSTER_COUNT: u16 = 2;

/// The ID of a FAT cluster.
/// TODO: Currently one cluster equals one block. Make it parametric!
#[repr(transparent)]
#[derive(
    Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
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
#[derive(Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[display("{_0}")]
pub struct BlockId(pub u16);

#[repr(transparent)]
#[derive(Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Deref, DerefMut, Display, From, Into, Debug, Clone, Copy, PartialEq, Eq)]
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
/// To be stored in the first block of the device.
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

pub const BOOT_SECTOR_SIZE: usize = std::mem::size_of::<BootSector>();

impl From<&BootSector> for [u8; BOOT_SECTOR_SIZE] {
    fn from(boot: &BootSector) -> Self {
        unsafe { std::mem::transmute_copy::<BootSector, Self>(boot) }
    }
}

/// On-disk file control block stored inside directory slots.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fcb {
    pub short_name: ShortName,
    pub attr: FcbAttr,
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
}

impl From<&Fcb> for [u8; FCB_SIZE] {
    fn from(fcb: &Fcb) -> Self {
        unsafe { std::mem::transmute_copy::<Fcb, Self>(fcb) }
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
}
