use super::*;

#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("parent/private");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("credential"), b"fixture-private").unwrap();
    (temp, root)
}

#[cfg(unix)]
fn capture(root: &Path) -> Inventory {
    Inventory::capture(&[root.into()]).unwrap()
}

#[cfg(unix)]
#[test]
fn private_inventory_accepts_nested_roots_and_detects_changed_bytes_names_and_modes() {
    use std::os::unix::fs::PermissionsExt;
    for change in 0..4 {
        let (_temp, root) = fixture();
        std::fs::create_dir(root.join("nested")).unwrap();
        let file = root.join("nested/key");
        std::fs::write(&file, b"fixture-only").unwrap();
        let inventory = Inventory::capture(&[root.clone(), file.clone()]).unwrap();
        inventory.recheck().unwrap();
        match change {
            0 => std::fs::write(&file, b"changed-fixture").unwrap(),
            1 => std::fs::rename(&file, root.join("renamed")).unwrap(),
            2 => std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o400)).unwrap(),
            _ => std::fs::write(root.join("new"), b"new-fixture").unwrap(),
        }
        assert!(inventory.recheck().is_err());
    }
}

#[cfg(unix)]
#[test]
fn private_inventory_refuses_external_and_internal_hard_links_before_and_after_capture() {
    for external in [false, true] {
        let (temp, root) = fixture();
        let inventory = capture(&root);
        let alias = if external {
            temp.path().join("alias")
        } else {
            root.join("alias")
        };
        std::fs::hard_link(root.join("credential"), &alias).unwrap();
        assert_eq!(
            Inventory::capture(std::slice::from_ref(&root)).unwrap_err(),
            Error::Unsafe
        );
        assert_eq!(inventory.recheck(), Err(Error::Unsafe));
        std::fs::remove_file(alias).unwrap();
        // A fresh capture may admit the now-unlinked object. Do not require the
        // filesystem clock to distinguish this rapid link/unlink round trip.
        // The live-alias refusals above are the invariant, not timestamp precision.
        capture(&root).recheck().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn private_inventory_never_follows_symlinks_or_opens_special_objects() {
    let (temp, root) = fixture();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(Inventory::capture(&[alias.join("credential")]).is_err());
    std::os::unix::fs::symlink(root.join("credential"), root.join("link")).unwrap();
    assert_eq!(
        Inventory::capture(std::slice::from_ref(&root)).unwrap_err(),
        Error::Unsafe
    );
    std::fs::remove_file(root.join("link")).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(root.join("socket")).unwrap();
    assert_eq!(Inventory::capture(&[root]).unwrap_err(), Error::Unsafe);
    drop(socket);
}

#[cfg(unix)]
#[test]
fn private_inventory_rejects_replaced_roots_and_ancestor_bindings() {
    for replace_parent in [false, true] {
        let (temp, root) = fixture();
        let inventory = capture(&root);
        if replace_parent {
            let parent = root.parent().unwrap();
            let old = temp.path().join("old-parent");
            std::fs::rename(parent, &old).unwrap();
            std::os::unix::fs::symlink(&old, parent).unwrap();
        } else {
            std::fs::rename(&root, temp.path().join("old")).unwrap();
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join("credential"), b"fixture-private").unwrap();
        }
        assert!(inventory.recheck().is_err());
    }
}

#[cfg(unix)]
#[test]
fn private_inventory_request_and_depth_limits_are_enforced() {
    assert_eq!(Inventory::capture(&[]).unwrap_err(), Error::Request);
    assert_eq!(
        Inventory::capture(&vec![PathBuf::from("/"); MAX_ROOTS + 1]).unwrap_err(),
        Error::Request
    );
    assert_eq!(
        Inventory::capture(&["relative".into()]).unwrap_err(),
        Error::Request
    );
    let (_temp, root) = fixture();
    assert_eq!(
        Inventory::capture(&[root.join("../private")]).unwrap_err(),
        Error::Request
    );
    let mut current = root.clone();
    for _ in 0..=MAX_DEPTH {
        current = current.join("d");
        std::fs::create_dir(&current).unwrap();
    }
    assert_eq!(Inventory::capture(&[root]).unwrap_err(), Error::Limit);
}

#[cfg(not(unix))]
#[test]
fn private_inventory_refuses_unsupported_platforms() {
    assert_eq!(
        Inventory::capture(&["fixture".into()]).unwrap_err(),
        Error::Unavailable
    );
}

#[cfg(unix)]
#[test]
fn private_inventory_enforces_unique_entry_limit_with_real_directory_entries() {
    let (_temp, root) = fixture();
    // Root and credential occupy two of the available entries.
    for i in 0..MAX_ENTRIES - 2 {
        std::fs::write(root.join(format!("entry-{i}")), b"").unwrap();
    }
    capture(&root).recheck().unwrap();
    std::fs::write(root.join("one-too-many"), b"").unwrap();
    assert_eq!(Inventory::capture(&[root]).unwrap_err(), Error::Limit);
}

#[cfg(unix)]
#[test]
fn private_inventory_enforces_path_byte_budget_before_entry_limit() {
    use std::os::unix::ffi::OsStrExt;
    let (_temp, root) = fixture();
    let mut nested = root.clone();
    for _ in 0..3 {
        nested = nested.join("d".repeat(180));
        std::fs::create_dir(&nested).unwrap();
    }
    let width = nested.as_os_str().as_bytes().len() + 181;
    let files = MAX_PATH_BYTES / width + 1;
    assert!(files + 5 < MAX_ENTRIES);
    for i in 0..files {
        std::fs::write(nested.join(format!("{i:05}{}", "f".repeat(175))), b"").unwrap();
    }
    assert_eq!(Inventory::capture(&[root]).unwrap_err(), Error::Limit);
}
