use std::io;
use std::path::Path;

use include_dir::{Dir, DirEntry, include_dir};

static TOOLCHAINS_ASSET: Dir = include_dir!("$CARGO_MANIFEST_DIR/asset/toolchains");
static PLATFORMS_ASSET: Dir = include_dir!("$CARGO_MANIFEST_DIR/asset/platforms");

pub fn extract_buck2_assets(dest: &Path) -> io::Result<()> {
    let toolchains_root = dest.join("toolchains");
    let platforms_root = dest.join("platforms");
    std::fs::create_dir_all(&toolchains_root)?;
    std::fs::create_dir_all(&platforms_root)?;
    extract_dir(&toolchains_root, &TOOLCHAINS_ASSET)?;
    extract_dir(&platforms_root, &PLATFORMS_ASSET)?;
    Ok(())
}

fn extract_dir(dest: &Path, dir: &Dir) -> io::Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub_dir) => {
                let target_dir = dest.join(sub_dir.path());
                std::fs::create_dir_all(&target_dir)?;
                extract_dir(dest, sub_dir)?;
            }
            DirEntry::File(file) => {
                let target_path = dest.join(file.path());
                if let Some(parent) = target_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(target_path, file.contents())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::extract_buck2_assets;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir() -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!(
            "cargo-buckal-assets-{}-{}",
            std::process::id(),
            nanos
        ));
        path
    }

    #[test]
    fn extract_buck2_assets_creates_expected_files() {
        let dest = unique_temp_dir();
        std::fs::create_dir_all(&dest).expect("failed to create temp dir");

        extract_buck2_assets(&dest).expect("failed to extract assets");

        assert!(dest.join("toolchains").is_dir());
        assert!(dest.join("platforms").is_dir());

        let toolchains_buck = dest.join("toolchains").join("BUCK");
        let platforms_buck = dest.join("platforms").join("BUCK");
        let demo_cxx = dest.join("toolchains").join("cxx").join("demo_cxx.bzl");
        let demo_rust = dest.join("toolchains").join("rust").join("demo_rust.bzl");

        assert!(toolchains_buck.is_file());
        assert!(platforms_buck.is_file());
        assert!(demo_cxx.is_file());
        assert!(demo_rust.is_file());

        let toolchains_contents =
            std::fs::read_to_string(&toolchains_buck).expect("read toolchains BUCK");
        assert!(!toolchains_contents.trim().is_empty());

        let platforms_contents =
            std::fs::read_to_string(&platforms_buck).expect("read platforms BUCK");
        assert!(!platforms_contents.trim().is_empty());

        std::fs::remove_dir_all(&dest).ok();
    }
}
