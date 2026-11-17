//! Exact immutable images, shared by Git restore and owned-journal materialization.
use sha1::{Digest, Sha1};
use sha2::Sha256;

pub(super) type Catalog = std::collections::BTreeMap<String, (String, String)>;
pub(super) const TREE_LIMIT: usize = 8 * 1024 * 1024;
pub(super) const GRAPH_LIMIT: usize = 64 * 1024 * 1024;

pub(super) fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || std::path::Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Invalid Git image path".into());
    }
    Ok(())
}
/// Parse raw Git tree records without treating binary object IDs as delimiters.
pub(super) fn tree_entries(
    mut bytes: &[u8],
    width: usize,
) -> Result<Vec<(String, String, String)>, String> {
    if !matches!(width, 20 | 32) {
        return Err("Invalid Git tree object width".into());
    }
    let mut entries = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    while !bytes.is_empty() {
        let space = bytes
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or("Malformed Git tree mode")?;
        let mode = std::str::from_utf8(&bytes[..space]).map_err(|_| "Invalid Git tree mode")?;
        let tail = &bytes[space + 1..];
        let nul = tail
            .iter()
            .position(|byte| *byte == 0)
            .ok_or("Unterminated Git tree path")?;
        let name = std::str::from_utf8(&tail[..nul])
            .map_err(|_| "Non-UTF-8 Git tree path is unsupported")?;
        validate_path(name)?;
        if name.contains('/') || !names.insert(name.to_owned()) {
            return Err("Ambiguous Git tree path".into());
        }
        let object = tail
            .get(nul + 1..nul + 1 + width)
            .ok_or("Truncated Git tree object")?;
        let mode = match mode {
            "40000" | "040000" => "040000",
            "100644" | "100755" | "120000" | "160000" => mode,
            _ => return Err("Unsupported Git tree mode".into()),
        };
        entries.push((
            name.into(),
            mode.into(),
            object.iter().map(|byte| format!("{byte:02x}")).collect(),
        ));
        if entries.len() > 65536 {
            return Err("Git tree entry limit exceeded".into());
        }
        bytes = &tail[nul + 1 + width..];
    }
    Ok(entries)
}

pub(super) fn verify(object: &str, bytes: &[u8]) -> Result<(), String> {
    verify_object(object, "blob", bytes)
}
pub(super) fn verify_object(object: &str, kind: &str, bytes: &[u8]) -> Result<(), String> {
    let header = format!("{kind} {}\0", bytes.len());
    let actual = match object.len() {
        40 => {
            let mut hash = Sha1::new();
            hash.update(header.as_bytes());
            hash.update(bytes);
            format!("{:x}", hash.finalize())
        }
        64 => {
            let mut hash = Sha256::new();
            hash.update(header.as_bytes());
            hash.update(bytes);
            format!("{:x}", hash.finalize())
        }
        _ => return Err("Invalid Git image object identity".into()),
    };
    if actual != object {
        return Err("Git snapshot image does not match its object identity".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_tree_records_preserve_literal_names_and_bound_object_width() {
        let name = ":(glob)*\tline\nname";
        let mut record = format!("100755 {name}\0").into_bytes();
        record.extend([0x11; 20]);
        assert_eq!(
            tree_entries(&record, 20).unwrap(),
            [(name.into(), "100755".into(), "11".repeat(20))]
        );
        assert!(tree_entries(&record, 32).is_err());
        assert!(tree_entries(&[record.clone(), record.clone()].concat(), 20).is_err());
        assert!(tree_entries(b"40000 ../outside\0", 20).is_err());
        assert!(validate_path("../outside").is_err());
        assert!(validate_path("a//b").is_err());
        let mut wide = b"40000 nested\0".to_vec();
        wide.extend([0; 32]);
        assert_eq!(
            tree_entries(&wide, 32).unwrap(),
            [("nested".into(), "040000".into(), "00".repeat(32))]
        );
    }
    #[test]
    fn git_object_hashes_are_checked_with_the_type_and_length_frame() {
        // Independent known Git empty-blob identities for both repository formats.
        verify("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", b"").unwrap();
        verify(
            "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
            b"",
        )
        .unwrap();
        assert!(verify("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391", b"changed").is_err());
    }
}
