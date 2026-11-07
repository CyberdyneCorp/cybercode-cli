//! Bind generated shadow ignore rules to their original user-repository input.
use super::config_inputs::{ConfigInputs, environment_digest};
use super::storage::{self, Result};
use crate::git::prepared::CaptureSettings;
use cap_std::fs::Dir;
use std::path::Path;

const LIMIT: u64 = 1024 * 1024;
const RECIPE: &str = "cyber-excludes.json";
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: u32,
    source: String,
    settings: CaptureSettings,
    skipped: Vec<String>,
    output: String,
}

fn source(common: &Path) -> Result<(ConfigInputs, Vec<u8>)> {
    let path = common.join("info/exclude");
    let mut proof = ConfigInputs::capture_files(vec![path.clone()])?;
    proof.bind_contexts(vec![common.to_path_buf()])?;
    let bytes = proof.bytes(&path)?;
    proof.verify()?;
    Ok((proof, bytes))
}

fn render(mut bytes: Vec<u8>, settings: &CaptureSettings, skipped: &[String]) -> Result<Vec<u8>> {
    if settings.ignore.len() > 4096 || skipped.len() > 4096 {
        return Err("Generated Git ignore input count exceeds limit".into());
    }
    bytes.push(b'\n');
    for pattern in &settings.ignore {
        append(&mut bytes, pattern)?;
    }
    for path in skipped {
        if path.contains(['\n', '\r', '\0']) {
            return Err("Unsupported generated Git ignore path".into());
        }
        append(&mut bytes, &format!("/{}", crate::git::escape(path)))?;
    }
    if bytes.len() as u64 > LIMIT {
        return Err("Generated Git ignore bytes exceed limit".into());
    }
    Ok(bytes)
}
fn append(bytes: &mut Vec<u8>, line: &str) -> Result<()> {
    if line.len() > 8192 || (bytes.len() + line.len() + 1) as u64 > LIMIT {
        return Err("Generated Git ignore bytes exceed limit".into());
    }
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    Ok(())
}

pub(crate) fn write(
    git_dir: &Path,
    common: &Path,
    settings: &CaptureSettings,
    skipped: &[String],
) -> Result<()> {
    let (proof, original) = source(common)?;
    let bytes = render(original, settings, skipped)?;
    let recipe = serde_json::to_vec(&Recipe {
        version: 1,
        source: proof.fingerprint()?,
        settings: settings.clone(),
        skipped: skipped.to_vec(),
        output: storage::digest(&bytes),
    })
    .map_err(storage::fail)?;
    if recipe.len() as u64 > LIMIT {
        return Err("Generated Git ignore recipe exceeds limit".into());
    }
    let root =
        Dir::open_ambient_dir(git_dir, cap_std::ambient_authority()).map_err(storage::fail)?;
    let info = storage::shared_child(&root, "info")?;
    proof.verify()?;
    publish(&info, "exclude", &bytes)?;
    publish(&info, RECIPE, &recipe)?;
    proof.verify()
}

fn publish(info: &Dir, name: &str, bytes: &[u8]) -> Result<()> {
    // Keep generations stable across unchanged capture. Existing links or corrupt
    // output are replaced through the directory descriptor, never followed.
    if storage::read(info, name, LIMIT).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    storage::atomic(info, name, bytes)
}

pub(crate) struct Observed {
    pub inputs: ConfigInputs,
    pub size_excluded: Vec<String>,
}

pub(crate) fn observe(
    git_dir: &Path,
    common: &Path,
    settings: &CaptureSettings,
) -> Result<Observed> {
    let root =
        Dir::open_ambient_dir(git_dir, cap_std::ambient_authority()).map_err(storage::fail)?;
    let info = storage::existing_child(&root, "info", false)?;
    let paths = vec![
        git_dir.join("info/exclude"),
        git_dir.join("info").join(RECIPE),
    ];
    let mut output = ConfigInputs::capture(paths.clone(), environment_digest()?)?;
    output.bind_contexts(vec![git_dir.join("info")])?;
    let encoded = storage::read(&info, RECIPE, LIMIT)?;
    let recipe: Recipe = serde_json::from_slice(&encoded).map_err(storage::fail)?;
    let (source, original) = source(common)?;
    if recipe.version != 1
        || recipe.settings != *settings
        || recipe.source != source.fingerprint()?
    {
        return Err("Generated Git ignore source or settings changed".into());
    }
    let expected = render(original, settings, &recipe.skipped)?;
    if recipe.output != storage::digest(&expected)
        || storage::read(&info, "exclude", LIMIT)? != expected
        || output.bytes(&paths[0])? != expected
        || output.bytes(&paths[1])? != encoded
    {
        return Err("Generated Git ignore recipe or output changed".into());
    }
    output.merge(source)?;
    Ok(Observed {
        inputs: output,
        size_excluded: recipe.skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture {
        _temp: tempfile::TempDir,
        common: std::path::PathBuf,
        shadow: std::path::PathBuf,
        settings: CaptureSettings,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let base = temp.path().canonicalize().unwrap();
            let common = base.join("common");
            let shadow = base.join("shadow");
            std::fs::create_dir_all(common.join("info")).unwrap();
            std::fs::create_dir(&shadow).unwrap();
            Self {
                _temp: temp,
                common,
                shadow,
                settings: CaptureSettings {
                    ignore: vec!["*.tmp".into()],
                    max_untracked: 65536,
                },
            }
        }
        fn write(&self) {
            write(
                &self.shadow,
                &self.common,
                &self.settings,
                &["big[1]".into()],
            )
            .unwrap();
        }
        fn proof(&self) -> Result<ConfigInputs> {
            observe(&self.shadow, &self.common, &self.settings).map(|observed| observed.inputs)
        }
    }

    #[test]
    fn unchanged_generation_retains_observations_and_missing_source_is_bound() {
        let fixture = Fixture::new();
        fixture.write();
        let proof = fixture.proof().unwrap();
        fixture.write();
        assert_eq!(fixture.proof().unwrap(), proof);
        proof.verify().unwrap();
        std::fs::write(fixture.common.join("info/exclude"), "").unwrap();
        assert!(proof.verify().is_err());
        assert!(fixture.proof().is_err());
        fixture.write();
        fixture.proof().unwrap().verify().unwrap();
    }

    #[test]
    fn generation_recipe_output_settings_and_common_directory_are_bound() {
        for kind in ["source", "output", "recipe", "settings", "common"] {
            let mut fixture = Fixture::new();
            std::fs::write(fixture.common.join("info/exclude"), "*.secret\n").unwrap();
            fixture.write();
            let proof = fixture.proof().unwrap();
            match kind {
                "source" => {
                    std::fs::write(fixture.common.join("info/exclude"), "changed\n").unwrap()
                }
                "output" => {
                    std::fs::write(fixture.shadow.join("info/exclude"), "changed\n").unwrap()
                }
                "recipe" => std::fs::write(fixture.shadow.join("info").join(RECIPE), "{}").unwrap(),
                "settings" => fixture.settings.max_untracked += 1,
                "common" => {
                    std::fs::rename(&fixture.common, fixture.common.with_extension("old")).unwrap();
                    std::fs::create_dir_all(fixture.common.join("info")).unwrap();
                    std::fs::write(fixture.common.join("info/exclude"), "*.secret\n").unwrap();
                }
                _ => unreachable!(),
            }
            assert!(fixture.proof().is_err(), "{kind}");
            if kind != "settings" {
                assert!(proof.verify().is_err(), "{kind}");
            }
        }
    }

    #[test]
    fn generated_output_links_are_replaced_without_following_and_parent_links_refuse() {
        let fixture = Fixture::new();
        fixture.write();
        let victim = fixture.common.join("victim");
        std::fs::write(&victim, "preserve").unwrap();
        for name in ["exclude", RECIPE] {
            let path = fixture.shadow.join("info").join(name);
            std::fs::remove_file(&path).unwrap();
            symlink(&victim, &path).unwrap();
            assert!(fixture.proof().is_err());
            fixture.write();
            assert!(
                !std::fs::symlink_metadata(path)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(std::fs::read(&victim).unwrap(), b"preserve");
        }
        std::fs::rename(fixture.shadow.join("info"), fixture.shadow.join("old-info")).unwrap();
        symlink(fixture.common.join("info"), fixture.shadow.join("info")).unwrap();
        assert!(write(&fixture.shadow, &fixture.common, &fixture.settings, &[]).is_err());
        assert!(fixture.proof().is_err());
        assert!(!fixture.common.join("info").join(RECIPE).exists());
    }

    #[test]
    fn raw_source_bytes_are_preserved_and_unsupported_sources_are_bounded() {
        let fixture = Fixture::new();
        let path = fixture.common.join("info/exclude");
        std::fs::write(&path, b"\xff\n").unwrap();
        fixture.write();
        assert!(
            std::fs::read(fixture.shadow.join("info/exclude"))
                .unwrap()
                .starts_with(b"\xff\n")
        );
        fixture.proof().unwrap().verify().unwrap();
        std::fs::write(&path, vec![0; LIMIT as usize + 1]).unwrap();
        assert!(write(&fixture.shadow, &fixture.common, &fixture.settings, &[]).is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(write(&fixture.shadow, &fixture.common, &fixture.settings, &[]).is_err());
    }
}
