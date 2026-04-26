//! File system abstractions and operations.

use bytemuck::Pod;
use bytemuck::Zeroable;
use chrono::DateTime;
use chrono::Datelike;
use chrono::NaiveDate;
use chrono::NaiveTime;
use chrono::Timelike;
use chrono::Utc;
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
pub const ROOT_DIR_START_CLUSTER: ClusterId = ClusterId(2);

/// The ID of a FAT cluster.
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

/// The ID of a logical block, aka sector in the current implementation.
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

/// A timezone-less date with reduced representable range packed into an [`u16`].
#[repr(transparent)]
#[derive(Deref, DerefMut, From, Into, Debug, Clone, Copy, PartialEq, Eq, Hash, Zeroable, Pod)]
pub struct U16Date(u16);

impl U16Date {
    pub const EMPTY: Self = Self(0);
}

impl TryFrom<NaiveDate> for U16Date {
    type Error = FsError;

    fn try_from(value: NaiveDate) -> Result<Self, Self::Error> {
        let year = value.year();
        if !(1980..=2107).contains(&year) {
            return Err(FsError::InvalidConfig(format!(
                "date {value} outside FAT range"
            )));
        }
        let year = u16::try_from(year - 1980).unwrap() & 0x7F; // 7 bits
        let month = u16::try_from(value.month()).unwrap() & 0x0F; // 4 bits
        let day = u16::try_from(value.day()).unwrap() & 0x1F; // 5 bits
        Ok(Self((year << 9) | (month << 5) | day))
    }
}

impl TryFrom<U16Date> for NaiveDate {
    type Error = FsError;

    fn try_from(value: U16Date) -> Result<Self, Self::Error> {
        if value == U16Date::EMPTY {
            return Err(FsError::CorruptFs("empty FAT date".to_string()));
        }
        let raw = u16::from(value);
        let year = 1980 + i32::from((raw >> 9) & 0x7F); // 7 bits
        let month = u32::from((raw >> 5) & 0x0F); // 4 bits
        let day = u32::from(raw & 0x1F); // 5 bits
        NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| FsError::CorruptFs(format!("invalid FAT date: {raw:#06x}")))
    }
}

impl TryFrom<DateTime<Utc>> for U16Date {
    type Error = FsError;

    fn try_from(value: DateTime<Utc>) -> Result<Self, Self::Error> {
        U16Date::try_from(value.naive_utc().date())
    }
}

impl fmt::Display for U16Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match NaiveDate::try_from(*self) {
            Ok(date) => write!(f, "{date}"),
            Err(_) => write!(f, "<unset>"),
        }
    }
}

/// A timezone-less time packed into an [`u16`].
#[repr(transparent)]
#[derive(Deref, DerefMut, From, Into, Debug, Clone, Copy, PartialEq, Eq, Hash, Zeroable, Pod)]
pub struct U16Time(u16);

impl U16Time {
    pub const EMPTY: Self = Self(0);
}

impl TryFrom<NaiveTime> for U16Time {
    type Error = FsError;

    fn try_from(value: NaiveTime) -> Result<Self, Self::Error> {
        let hour = u16::try_from(value.hour()).unwrap() & 0x1F; // 5 bits
        let minute = u16::try_from(value.minute()).unwrap() & 0x3F; // 6 bits
        let second_div2 = u16::try_from(value.second() >> 1).unwrap() & 0x1F; // 5 bits
        Ok(Self((hour << 11) | (minute << 5) | second_div2))
    }
}

impl TryFrom<U16Time> for NaiveTime {
    type Error = FsError;

    fn try_from(value: U16Time) -> Result<Self, Self::Error> {
        if value == U16Time::EMPTY {
            return Err(FsError::CorruptFs("empty FAT time".to_string()));
        }
        let raw = u16::from(value);
        let hour = u32::from((raw >> 11) & 0x1F); // 5 bits
        let minute = u32::from((raw >> 5) & 0x3F); // 6 bits
        let second = u32::from(raw & 0x1F) << 1; // 5 bits
        NaiveTime::from_hms_opt(hour, minute, second)
            .ok_or_else(|| FsError::CorruptFs(format!("invalid FAT time: {raw:#06x}")))
    }
}

impl TryFrom<DateTime<Utc>> for U16Time {
    type Error = FsError;

    fn try_from(value: DateTime<Utc>) -> Result<Self, Self::Error> {
        Self::try_from(value.naive_utc().time())
    }
}

impl fmt::Display for U16Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match NaiveTime::try_from(*self) {
            Ok(time) => write!(f, "{}", time.format("%H:%M:%S")),
            Err(_) => write!(f, "<unset>"),
        }
    }
}

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
    /// Block size of this FS.
    pub block_size: u16,
    /// Number of blocks of the whole disk.
    pub block_count: u16,
    /// Number of blocks per data cluster.
    pub blocks_per_cluster: u16,
    /// Starting block ID of the first FAT.
    pub fat_start_block: BlockId,
    /// Number of blocks allocated for each FAT.
    pub fat_block_count: u16,
    /// Number of copies of FAT.
    pub fat_copies: u16,
    /// Starting position of the first data cluster.
    pub data_start_block: BlockId,
    /// Starting cluster ID of the root directory.
    pub root_dir_start_cluster: ClusterId,
}

impl BootSector {
    pub const SIZE: usize = std::mem::size_of::<BootSector>();

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }

    pub fn read_from_prefix(bytes: &[u8]) -> Result<Self, FsError> {
        let bytes = bytes
            .get(..Self::SIZE)
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
    pub mtime: U16Time,
    pub mdate: U16Date,
    pub start_cluster: ClusterId,
    pub size: u32,
}

impl Fcb {
    pub const SIZE: usize = std::mem::size_of::<Fcb>();

    pub(crate) fn new(
        name: &str,
        kind: NodeKind,
        start_cluster: ClusterId,
        size: u32,
        mdatetime: DateTime<Utc>,
    ) -> Result<Self, FsError> {
        Ok(Self {
            short_name: ShortName::try_from(name)?,
            attr: kind.into(),
            reserved: 0,
            mtime: mdatetime.try_into()?,
            mdate: mdatetime.try_into()?,
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
            .get_mut(..Fcb::SIZE)
            .ok_or_else(|| FsError::CorruptFs("fcb slot shorter than expected".to_string()))?;
        dst.copy_from_slice(self.as_bytes());
        Ok(())
    }

    pub(crate) fn set_mdatetime(&mut self, mdatetime: DateTime<Utc>) -> Result<(), FsError> {
        self.mtime = mdatetime.try_into()?;
        self.mdate = mdatetime.try_into()?;
        Ok(())
    }

    pub(crate) fn touch(&mut self) -> Result<(), FsError> {
        self.set_mdatetime(Utc::now())?;
        Ok(())
    }
}

impl TryFrom<&[u8]> for Fcb {
    type Error = FsError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = value
            .get(..Fcb::SIZE)
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
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId::FREE, 0, Utc::now()).unwrap();
        assert_eq!(fcb.attr, FcbAttr::FILE);
        assert_eq!(fcb.start_cluster, ClusterId::FREE);
        assert_eq!(fcb.size, 0);
        assert_eq!(fcb.short_name(), "A.TXT");
        assert_ne!(fcb.mtime, U16Time::EMPTY);
        assert_ne!(fcb.mdate, U16Date::EMPTY);
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
        };
        assert_eq!(boot.as_bytes().len(), BootSector::SIZE);
        assert_eq!(BootSector::read_from_prefix(boot.as_bytes()).unwrap(), boot);
    }

    #[test]
    fn fcb_bytes_round_trip() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId(7), 123, Utc::now()).unwrap();
        assert_eq!(fcb.as_bytes().len(), Fcb::SIZE);
        assert_eq!(Fcb::try_from(fcb.as_bytes()).unwrap(), fcb);
    }

    #[test]
    fn write_to_slice_rejects_short_buffer() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId::FREE, 0, Utc::now()).unwrap();
        let mut short = [0u8; Fcb::SIZE - 1];
        assert!(fcb.write_to_slice(&mut short).is_err());
    }

    #[test]
    fn u16_date_converts_both_ways() {
        let date = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap();
        let encoded = U16Date::try_from(date).unwrap();
        assert_eq!(NaiveDate::try_from(encoded).unwrap(), date);
    }

    #[test]
    fn u16_time_converts_both_ways() {
        let time = NaiveTime::from_hms_opt(12, 34, 56).unwrap();
        let encoded = U16Time::try_from(time).unwrap();
        assert_eq!(NaiveTime::try_from(encoded).unwrap(), time);
        // Should even out the LSB of second
        let time_odd = NaiveTime::from_hms_opt(12, 34, 57).unwrap();
        let encoded_odd = U16Time::try_from(time_odd).unwrap();
        assert_eq!(NaiveTime::try_from(encoded_odd).unwrap(), time);
    }
}
