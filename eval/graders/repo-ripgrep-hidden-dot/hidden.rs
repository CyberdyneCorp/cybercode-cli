#![cfg(unix)]
use std::collections::BTreeSet;
use std::path::{Path,PathBuf};
#[cfg(target_os="linux")]
use std::os::unix::ffi::OsStringExt;
use ignore::WalkBuilder;

struct Scratch(PathBuf);
impl Drop for Scratch {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}

fn paths(root:&Path,include_hidden:bool)->BTreeSet<PathBuf> {
    let mut builder=WalkBuilder::new(root);
    builder.hidden(!include_hidden);
    builder.build().map(|entry|entry.unwrap().path().strip_prefix(root).unwrap().to_path_buf())
        .filter(|p|!p.as_os_str().is_empty()).collect()
}

#[test]
fn trailing_dots_are_real_names_and_hidden_override_is_preserved() {
    let unique=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let scratch=Scratch(std::env::temp_dir().join(format!("dot-eval-{}-{unique}",std::process::id())));
    let root=&scratch.0;std::fs::create_dir_all(root).unwrap();
    let names=["visible","visible.","visible..","prefix.hidden.",".hidden",".hidden.",".hidden..","...","...."];
    for name in names {std::fs::write(root.join(name),b"data").unwrap();}
    std::fs::create_dir_all(root.join(".directory.")).unwrap();
    std::fs::write(root.join(".directory./inside"),b"data").unwrap();
    // APFS cannot create arbitrary invalid-UTF-8 names. Exercise those on Linux.
    #[cfg(target_os="linux")]
    let non_utf8={
        let name=std::ffi::OsString::from_vec(b".hidden\xff.".to_vec());
        std::fs::write(root.join(&name),b"data").unwrap();name
    };
    let visible:BTreeSet<_>=["visible","visible.","visible..","prefix.hidden."].into_iter().map(PathBuf::from).collect();
    assert_eq!(paths(root,false),visible);
    let mut all:BTreeSet<_>=names.into_iter().map(PathBuf::from).collect();
    all.insert(PathBuf::from(".directory."));all.insert(PathBuf::from(".directory./inside"));
    #[cfg(target_os="linux")]
    all.insert(non_utf8.into());
    assert_eq!(paths(root,true),all);
}
