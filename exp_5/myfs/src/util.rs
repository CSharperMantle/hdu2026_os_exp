use crate::{BootSector, ClusterId, Fcb, FcbAttr, FsError};

pub const BOOT_SECTOR_SIZE: usize = 18;
pub const FCB_SERIALIZED_SIZE: usize = 22;
pub const DIR_ENTRY_SIZE: usize = 32;
pub const SLOT_UNUSED: u8 = 0x00;
pub const SLOT_DELETED: u8 = 0xE5;

pub(crate) enum DirSlot {
    Unused,
    Deleted,
    Occupied(Fcb),
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
    if !base.chars().all(valid_short_char) || !ext.chars().all(valid_short_char) {
        return Err(FsError::InvalidName(name.to_string()));
    }

    let mut base_out = [b' '; 8];
    let mut ext_out = [b' '; 3];
    base_out[..base.len()].copy_from_slice(base.as_bytes());
    ext_out[..ext.len()].copy_from_slice(ext.as_bytes());
    Ok((base_out, ext_out))
}

pub(crate) fn valid_short_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

pub(crate) fn decode_name_part(bytes: &[u8]) -> String {
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b' ' {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

pub(crate) fn serialize_boot_sector(boot: &BootSector) -> [u8; BOOT_SECTOR_SIZE] {
    let mut out = [0u8; BOOT_SECTOR_SIZE];
    out[0..2].copy_from_slice(&boot.block_size.to_le_bytes());
    out[2..4].copy_from_slice(&boot.block_count.to_le_bytes());
    out[4..6].copy_from_slice(&boot.blocks_per_cluster.to_le_bytes());
    out[6..8].copy_from_slice(&boot.fat_start_block.0.to_le_bytes());
    out[8..10].copy_from_slice(&boot.fat_block_count.to_le_bytes());
    out[10..12].copy_from_slice(&boot.fat_copies.to_le_bytes());
    out[12..14].copy_from_slice(&boot.data_start_block.0.to_le_bytes());
    out[14..16].copy_from_slice(&boot.root_dir_start_cluster.0.to_le_bytes());
    out[16..18].copy_from_slice(&boot.root_dir_cluster_count.to_le_bytes());
    out
}

pub(crate) fn serialize_fcb(fcb: &Fcb) -> [u8; FCB_SERIALIZED_SIZE] {
    let mut out = [0u8; FCB_SERIALIZED_SIZE];
    out[0..8].copy_from_slice(&fcb.file_name);
    out[8..11].copy_from_slice(&fcb.ext_name);
    out[11] = fcb.attr.0;
    out[12..14].copy_from_slice(&fcb.ctime.to_le_bytes());
    out[14..16].copy_from_slice(&fcb.cdate.to_le_bytes());
    out[16..18].copy_from_slice(&fcb.start_cluster.0.to_le_bytes());
    out[18..22].copy_from_slice(&fcb.size.to_le_bytes());
    out
}

pub(crate) fn parse_dir_slot(slot: &[u8]) -> Result<DirSlot, FsError> {
    if slot.len() < DIR_ENTRY_SIZE {
        return Err(FsError::CorruptFs(
            "directory slot shorter than expected".to_string(),
        ));
    }
    match slot[0] {
        SLOT_UNUSED => Ok(DirSlot::Unused),
        SLOT_DELETED => Ok(DirSlot::Deleted),
        _ => {
            let mut file_name = [0u8; 8];
            let mut ext_name = [0u8; 3];
            file_name.copy_from_slice(&slot[0..8]);
            ext_name.copy_from_slice(&slot[8..11]);
            let fcb = Fcb {
                file_name,
                ext_name,
                attr: FcbAttr(slot[11]),
                ctime: u16::from_le_bytes([slot[12], slot[13]]),
                cdate: u16::from_le_bytes([slot[14], slot[15]]),
                start_cluster: ClusterId(u16::from_le_bytes([slot[16], slot[17]])),
                size: u32::from_le_bytes([slot[18], slot[19], slot[20], slot[21]]),
            };
            Ok(DirSlot::Occupied(fcb))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, BootSector, NodeKind};

    #[test]
    fn normalize_component_enforces_83_names() {
        assert_eq!(normalize_component("readme.txt").unwrap(), "README.TXT");
        assert!(normalize_component("too_long_name.txt").is_err());
        assert!(normalize_component("bad-name.txt").is_err());
        assert!(normalize_component(".").is_err());
    }

    #[test]
    fn boot_sector_serialization_writes_expected_fields() {
        let boot = BootSector {
            block_size: 1024,
            block_count: 128,
            blocks_per_cluster: 1,
            fat_start_block: BlockId(1),
            fat_block_count: 1,
            fat_copies: 2,
            data_start_block: BlockId(3),
            root_dir_start_cluster: ClusterId(2),
            root_dir_cluster_count: 2,
        };
        let bytes = serialize_boot_sector(&boot);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 1024);
        assert_eq!(u16::from_le_bytes([bytes[6], bytes[7]]), 1);
        assert_eq!(u16::from_le_bytes([bytes[14], bytes[15]]), 2);
    }

    #[test]
    fn fcb_slot_round_trip_parses_back() {
        let fcb = Fcb::new("A.TXT", NodeKind::File, ClusterId(7), 123).unwrap();
        let mut slot = [0u8; DIR_ENTRY_SIZE];
        slot[..FCB_SERIALIZED_SIZE].copy_from_slice(&serialize_fcb(&fcb));
        match parse_dir_slot(&slot).unwrap() {
            DirSlot::Occupied(parsed) => assert_eq!(parsed, fcb),
            _ => panic!("expected occupied dir slot"),
        }
    }

    #[test]
    fn parse_dir_slot_recognizes_unused_and_deleted() {
        let unused = [0u8; DIR_ENTRY_SIZE];
        assert!(matches!(parse_dir_slot(&unused).unwrap(), DirSlot::Unused));

        let mut deleted = [0u8; DIR_ENTRY_SIZE];
        deleted[0] = SLOT_DELETED;
        assert!(matches!(
            parse_dir_slot(&deleted).unwrap(),
            DirSlot::Deleted
        ));
    }
}
