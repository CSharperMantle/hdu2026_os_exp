//! File name and path utilities.

use std::fmt;

use crate::FCB_SIZE;
use crate::Fcb;
use crate::FsError;

pub(crate) trait IsShortCompatible {
    /// Can this char be stored in a [`ShortName`]?
    fn is_short_compatible(&self) -> bool;
}

impl IsShortCompatible for char {
    fn is_short_compatible(&self) -> bool {
        self.is_ascii_alphanumeric() || *self == '_'
    }
}

#[derive(Debug, Clone)]
pub(crate) enum DirSlot {
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
            _ => {
                let mut buf = [0u8; FCB_SIZE];
                buf.copy_from_slice(&value[0..FCB_SIZE]);
                // SAFETY: Since the size of the buffers are the same, this should not be a problem on a native-endianness system.
                let fcb = unsafe { std::mem::transmute::<[u8; FCB_SIZE], Fcb>(buf) };
                Ok(DirSlot::Occupied(fcb))
            }
        }
    }
}

impl DirSlot {
    pub const SLOT_UNUSED: u8 = 0x00;
    pub const SLOT_DELETED: u8 = 0xE5;
}

fn spaced_to_string(bytes: &[u8]) -> String {
    let end = match bytes.iter().position(|ch| *ch == b' ') {
        None => bytes.len(),
        Some(p) => p,
    };
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// A file name (short name) used in [`MyFileSystem`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortName {
    pub base: [u8; Self::BASE_SIZE],
    pub ext: [u8; Self::EXT_SIZE],
}

impl fmt::Display for ShortName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base = spaced_to_string(&self.base);
        let ext = spaced_to_string(&self.ext);
        if ext.is_empty() {
            write!(f, "{}", base)
        } else {
            write!(f, "{}.{}", base, ext)
        }
    }
}

impl TryFrom<&str> for ShortName {
    type Error = FsError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let upper = value.to_ascii_uppercase();
        let mut parts = upper.split('.');
        let base = parts
            .next()
            .filter(|part| !part.is_empty())
            .ok_or_else(|| FsError::InvalidName(value.to_string()))?;
        let ext = parts.next().unwrap_or("");
        if parts.next().is_some() {
            return Err(FsError::InvalidName(value.to_string()));
        }
        if base.len() > Self::BASE_SIZE || ext.len() > Self::EXT_SIZE {
            return Err(FsError::InvalidName(value.to_string()));
        }
        if !base
            .chars()
            .chain(ext.chars())
            .all(|ch| ch.is_short_compatible())
        {
            return Err(FsError::InvalidName(value.to_string()));
        }

        let mut base_out = [b' '; Self::BASE_SIZE];
        let mut ext_out = [b' '; Self::EXT_SIZE];
        base_out[..base.len()].copy_from_slice(base.as_bytes());
        ext_out[..ext.len()].copy_from_slice(ext.as_bytes());
        Ok(ShortName {
            base: base_out,
            ext: ext_out,
        })
    }
}

impl ShortName {
    pub const BASE_SIZE: usize = 8;
    pub const EXT_SIZE: usize = 3;
}

pub(crate) fn normalize_component(component: &str) -> Result<String, FsError> {
    if component == "." || component == ".." || component.is_empty() {
        return Err(FsError::InvalidName(component.to_string()));
    }
    let _ = ShortName::try_from(component)?;
    Ok(component.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_component_enforces_83_names() {
        assert_eq!(normalize_component("readme.txt").unwrap(), "README.TXT");
        assert!(normalize_component("too_long_name.txt").is_err());
        assert!(normalize_component("bad-name.txt").is_err());
        assert!(normalize_component(".").is_err());
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
}
