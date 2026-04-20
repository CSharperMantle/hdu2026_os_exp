use crate::{FCB_SIZE, Fcb, FsError};

pub(crate) trait IsShort {
    fn is_short(&self) -> bool;
}

impl IsShort for char {
    fn is_short(&self) -> bool {
        self.is_ascii_alphanumeric() || *self == '_'
    }
}

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

pub(crate) fn normalize_component(component: &str) -> Result<String, FsError> {
    if component == "." || component == ".." || component.is_empty() {
        return Err(FsError::InvalidName(component.to_string()));
    }
    let _ = encode_short_name(component)?;
    Ok(component.to_ascii_uppercase())
}

pub(crate) fn encode_short_name(name: &str) -> Result<([u8; 8], [u8; 3]), FsError> {
    let upper = name.to_ascii_uppercase();
    let mut parts = upper.split('.');
    let base = parts
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| FsError::InvalidName(name.to_string()))?;
    let ext = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return Err(FsError::InvalidName(name.to_string()));
    }
    if base.len() > 8 || ext.len() > 3 {
        return Err(FsError::InvalidName(name.to_string()));
    }
    if !base.chars().chain(ext.chars()).all(|ch| ch.is_short()) {
        return Err(FsError::InvalidName(name.to_string()));
    }

    let mut base_out = [b' '; 8];
    let mut ext_out = [b' '; 3];
    base_out[..base.len()].copy_from_slice(base.as_bytes());
    ext_out[..ext.len()].copy_from_slice(ext.as_bytes());
    Ok((base_out, ext_out))
}

pub(crate) fn decode_name_part(bytes: &[u8]) -> String {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
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
