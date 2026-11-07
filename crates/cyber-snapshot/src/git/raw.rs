//! Raw working-tree graphs stay reachable alongside the canonical tree.
use super::{Shadow, images};
use crate::copy::git_capture::RawImage;
use std::collections::BTreeMap;

impl Shadow {
    pub(crate) async fn changed_native(&self, from: &str, to: &str) -> Result<Vec<String>, String> {
        let excluded = self.excluded_between(from, to).await?;
        let before = self.native_image_catalog(from).await?;
        let after = self.native_image_catalog(to).await?;
        let names: std::collections::BTreeSet<_> =
            before.keys().chain(after.keys()).cloned().collect();
        Ok(names
            .into_iter()
            .filter(|path| {
                before.get(path) != after.get(path) && !super::history::excludes(&excluded, path)
            })
            .collect())
    }

    pub(super) async fn write_raw_capture(
        &self,
        files: &BTreeMap<String, RawImage>,
    ) -> Result<String, String> {
        let mut directories: BTreeMap<String, Vec<u8>> = BTreeMap::from([(String::new(), vec![])]);
        for (path, image) in files {
            images::validate_path(path)?;
            let object = String::from_utf8(
                self.input(
                    &["hash-object", "-w", "--stdin", "--no-filters"],
                    &image.bytes,
                )
                .await?,
            )
            .map_err(|_| "Invalid raw Git blob identity")?
            .trim()
            .to_owned();
            images::verify(&object, &image.bytes)?;
            let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", path));
            let mut ancestor = parent;
            while !ancestor.is_empty() {
                directories.entry(ancestor.into()).or_default();
                ancestor = ancestor.rsplit_once('/').map_or("", |(parent, _)| parent);
            }
            directories
                .entry(parent.into())
                .or_default()
                .extend(format!("{} blob {object}\t{leaf}\0", image.mode).bytes());
        }
        let mut order: Vec<_> = directories.keys().cloned().collect();
        order.sort_by_key(|path| {
            std::cmp::Reverse(path.matches('/').count() + usize::from(!path.is_empty()))
        });
        for directory in order {
            let bytes = directories
                .remove(&directory)
                .ok_or("Missing raw Git directory")?;
            let object = self.write_tree_records(&bytes).await?;
            if directory.is_empty() {
                return Ok(object);
            }
            let (parent, leaf) = directory.rsplit_once('/').unwrap_or(("", &directory));
            directories
                .get_mut(parent)
                .ok_or("Missing raw Git parent")?
                .extend(format!("040000 tree {object}\t{leaf}\0").bytes());
        }
        Err("Missing raw Git root".into())
    }
    pub(super) async fn write_tree_records(&self, records: &[u8]) -> Result<String, String> {
        let object = String::from_utf8(self.input(&["mktree", "-z"], records).await?)
            .map_err(|_| "Invalid raw Git tree identity")?
            .trim()
            .to_owned();
        let bytes = self
            .run_bounded(&["cat-file", "tree", &object], images::TREE_LIMIT)
            .await?;
        images::verify_object(&object, "tree", &bytes)?;
        Ok(object)
    }
}
