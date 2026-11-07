//! Exact immutable images, shared by Git restore and owned-journal materialization.
use sha1::{Digest, Sha1};
use sha2::Sha256;

pub(super) fn entry(listing: &[u8], path: &str) -> Result<Option<(String, String)>, String> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || std::path::Path::new(path)
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Invalid Git image path".into());
    }
    if listing.is_empty() {
        return Ok(None);
    }
    let record = listing
        .strip_suffix(&[0])
        .ok_or("Unterminated Git image listing")?;
    if record.contains(&0) {
        return Err("Ambiguous Git image listing".into());
    }
    let split = record
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or("Malformed Git image listing")?;
    if &record[split + 1..] != path.as_bytes() {
        return Err("Git image path differs from the requested literal path".into());
    }
    let metadata =
        std::str::from_utf8(&record[..split]).map_err(|_| "Invalid Git image metadata")?;
    let fields: Vec<_> = metadata.split(' ').collect();
    if let ["040000", "tree", object] = fields.as_slice() {
        if !oid(object) {
            return Err("Invalid Git directory image identity".into());
        }
        return Ok(None); // A directory is not a file image at this exact path.
    }
    let [mode, "blob", object] = fields.as_slice() else {
        return Err("Unsupported Git image entry".into());
    };
    if !matches!(*mode, "100644" | "100755" | "120000") || !oid(object) {
        return Err("Invalid Git image mode or object identity".into());
    }
    Ok(Some(((*mode).into(), (*object).into())))
}
fn oid(object: &str) -> bool {
    matches!(object.len(), 40 | 64)
        && object
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(super) fn verify(object: &str, bytes: &[u8]) -> Result<(), String> {
    let header = format!("blob {}\0", bytes.len());
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
    fn exact_nul_records_preserve_unusual_paths_and_refuse_ambiguous_or_foreign_images() {
        let object = "a".repeat(40);
        let path = ":(glob)*\tline\nname";
        let record = format!("100755 blob {object}\t{path}\0");
        assert_eq!(
            entry(record.as_bytes(), path).unwrap(),
            Some(("100755".into(), object.clone()))
        );
        assert_eq!(entry(b"", path).unwrap(), None);
        assert!(entry(b"", "../outside").is_err());
        assert_eq!(
            entry(format!("040000 tree {object}\t{path}\0").as_bytes(), path).unwrap(),
            None
        );
        assert!(entry(record.as_bytes(), "different").is_err());
        assert!(entry(record.trim_end_matches('\0').as_bytes(), path).is_err());
        assert!(entry(format!("{record}{record}").as_bytes(), path).is_err());
        assert!(entry(format!("160000 commit {object}\t{path}\0").as_bytes(), path).is_err());
        assert!(entry(format!("100644 blob invalid\t{path}\0").as_bytes(), path).is_err());
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
