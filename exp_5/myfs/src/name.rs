//! File name and path utilities.

use derive_more::Deref;
use derive_more::DerefMut;
use std::fmt;

use crate::FsError;

trait IsShortCompatible {
    /// Can this char be stored in a [`ShortName`]?
    fn is_short_compatible(&self) -> bool;
}

impl IsShortCompatible for char {
    fn is_short_compatible(&self) -> bool {
        self.is_ascii_alphanumeric() || *self == '_' || *self == ' '
    }
}

/// An [`u8`] array filled with `b' '` by default.
///
/// ## See also
///
/// ["Design of the FAT file system"](https://en.wikipedia.org/wiki/Design_of_the_FAT_file_system#Directory_table), Wikipedia.
#[repr(transparent)]
#[derive(Deref, DerefMut, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpacedCharBuf<const SIZE: usize>([u8; SIZE]);

impl<const SIZE: usize> SpacedCharBuf<SIZE> {
    pub const SPACE: u8 = b' ';

    /// FAT-visible length.
    ///
    /// Trailing spaces are padding. Internal spaces are part of the name.
    pub fn len(&self) -> usize {
        self.iter()
            .rposition(|ch| *ch != Self::SPACE)
            .map_or(0, |idx| idx + 1)
    }

    pub fn is_empty(&self) -> bool {
        self.iter().all(|ch| *ch == Self::SPACE)
    }
}

impl<const SIZE: usize> Default for SpacedCharBuf<SIZE> {
    fn default() -> Self {
        Self([Self::SPACE; SIZE])
    }
}

impl<const SIZE: usize> AsRef<[u8; SIZE]> for SpacedCharBuf<SIZE> {
    fn as_ref(&self) -> &[u8; SIZE] {
        &self.0
    }
}

impl<const SIZE: usize> AsMut<[u8; SIZE]> for SpacedCharBuf<SIZE> {
    fn as_mut(&mut self) -> &mut [u8; SIZE] {
        &mut self.0
    }
}

impl<const SIZE: usize> AsRef<[u8]> for SpacedCharBuf<SIZE> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const SIZE: usize> AsMut<[u8]> for SpacedCharBuf<SIZE> {
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl<const SIZE: usize> From<[u8; SIZE]> for SpacedCharBuf<SIZE> {
    fn from(value: [u8; SIZE]) -> Self {
        Self(value)
    }
}

impl<const SIZE: usize> From<SpacedCharBuf<SIZE>> for [u8; SIZE] {
    fn from(value: SpacedCharBuf<SIZE>) -> Self {
        value.0
    }
}

impl<const SIZE: usize> From<SpacedCharBuf<SIZE>> for String {
    fn from(value: SpacedCharBuf<SIZE>) -> Self {
        let end = value.len();
        String::from_utf8_lossy(&value.0[..end]).into_owned()
    }
}

impl<const SIZE: usize> fmt::Display for SpacedCharBuf<SIZE> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from(*self))
    }
}

impl<const SIZE: usize> TryFrom<&str> for SpacedCharBuf<SIZE> {
    type Error = FsError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.len() > SIZE || !value.chars().all(|ch| ch.is_short_compatible()) {
            return Err(FsError::InvalidName(value.to_string()));
        }
        let mut out = Self::default();
        out[..value.len()].copy_from_slice(value.as_bytes());
        Ok(out)
    }
}

/// A file name (short name) used in [`MyFileSystem`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortName {
    pub base: SpacedCharBuf<{ Self::BASE_SIZE }>,
    pub ext: SpacedCharBuf<{ Self::EXT_SIZE }>,
}

impl fmt::Display for ShortName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base = self.base.to_string();
        let ext = self.ext.to_string();
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
        if !base
            .chars()
            .chain(ext.chars())
            .all(|ch| ch.is_short_compatible())
        {
            return Err(FsError::InvalidName(value.to_string()));
        }
        Ok(ShortName {
            base: SpacedCharBuf::try_from(base)?,
            ext: SpacedCharBuf::try_from(ext)?,
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
    fn spaced_char_buf_default_is_all_spaces() {
        let buf = SpacedCharBuf::<8>::default();
        assert_eq!(<[u8; 8]>::from(buf), [b' '; 8]);
    }

    #[test]
    fn spaced_char_buf_try_from_and_display_work() {
        let buf = SpacedCharBuf::<8>::try_from("README").unwrap();
        assert_eq!(buf.to_string(), "README");
        assert_eq!(buf.len(), 6);
        assert_eq!(buf[0], b'R');
        assert_eq!(buf[6], b' ');
        assert!(SpacedCharBuf::<3>::try_from("TOOLONG").is_err());
        assert!(SpacedCharBuf::<8>::try_from("BAD-NAME").is_err());
    }

    #[test]
    fn spaced_char_buf_keeps_internal_spaces_but_trims_trailing_spaces() {
        let buf = SpacedCharBuf::<8>::from(*b"A B     ");
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.to_string(), "A B");
        assert!(!buf.is_empty());
    }

    #[test]
    fn spaced_char_buf_all_spaces_is_empty_and_zero_len() {
        let buf = SpacedCharBuf::<8>::default();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());
        assert_eq!(buf.to_string(), "");
    }

    #[test]
    fn normalize_component_enforces_83_names() {
        assert_eq!(normalize_component("readme.txt").unwrap(), "README.TXT");
        assert!(normalize_component("too_long_name.txt").is_err());
        assert!(normalize_component("bad-name.txt").is_err());
        assert!(normalize_component(".").is_err());
    }

    #[test]
    fn short_name_uses_spaced_char_buf() {
        let short = ShortName::try_from("readme.txt").unwrap();
        assert_eq!(short.base.to_string(), "README");
        assert_eq!(short.ext.to_string(), "TXT");
        assert_eq!(short.to_string(), "README.TXT");
    }
}
