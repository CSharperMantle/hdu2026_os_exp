//! File allocation table (FAT) abstractions.

use std::fmt;

use crate::ClusterId;

/// A type-safe FAT entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FatEntry {
    Free,
    EndOfChain,
    Next(ClusterId),
}

impl FatEntry {
    pub const SIZE: usize = std::mem::size_of::<u16>();
}

impl From<FatEntry> for u16 {
    fn from(value: FatEntry) -> Self {
        match value {
            FatEntry::Free => ClusterId::FAT_FREE,
            FatEntry::EndOfChain => ClusterId::FAT_EOC,
            FatEntry::Next(cluster) => cluster.into(),
        }
    }
}

impl From<u16> for FatEntry {
    fn from(value: u16) -> Self {
        match value {
            ClusterId::FAT_FREE => FatEntry::Free,
            ClusterId::FAT_EOC => FatEntry::EndOfChain,
            other => FatEntry::Next(other.into()),
        }
    }
}

impl fmt::Display for FatEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FatEntry::Free => write!(f, "FREE"),
            FatEntry::EndOfChain => write!(f, "EOC"),
            FatEntry::Next(next) => write!(f, "{}", next),
        }
    }
}

pub(crate) fn fat_offset(cluster: ClusterId) -> usize {
    usize::from(u16::from(cluster)) * FatEntry::SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fat_entry_converts_both_ways() {
        assert_eq!(u16::from(FatEntry::Free), ClusterId::FAT_FREE);
        assert_eq!(u16::from(FatEntry::EndOfChain), ClusterId::FAT_EOC);
        assert_eq!(u16::from(FatEntry::Next(ClusterId(7))), 7);
        assert_eq!(FatEntry::from(ClusterId::FAT_FREE), FatEntry::Free);
        assert_eq!(FatEntry::from(ClusterId::FAT_EOC), FatEntry::EndOfChain);
        assert_eq!(FatEntry::from(11), FatEntry::Next(ClusterId(11)));
    }
}
