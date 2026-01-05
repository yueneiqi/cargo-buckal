use std::collections::HashMap;

use cargo_lock::{Checksum, Lockfile};
use cargo_metadata::{MetadataCommand, Node, Package, PackageId, camino::Utf8PathBuf};

use crate::{config::RepoConfig, utils::UnwrapOrExit};

pub struct BuckalContext {
    pub root: Package,
    pub nodes_map: HashMap<PackageId, Node>,
    pub packages_map: HashMap<PackageId, Package>,
    pub checksums_map: HashMap<String, Checksum>,
    pub workspace_root: Utf8PathBuf,
    // whether to skip merging manual changes in BUCK files
    pub no_merge: bool,
    pub separate: bool,
    // repository configuration
    pub repo_config: RepoConfig,
}

impl BuckalContext {
    pub fn new() -> Self {
        let cargo_metadata = MetadataCommand::new().exec().unwrap();
        let root = cargo_metadata.root_package().unwrap().to_owned();
        let packages_map = cargo_metadata
            .packages
            .into_iter()
            .map(|p| (p.id.to_owned(), p))
            .collect::<HashMap<_, _>>();
        let resolve = cargo_metadata.resolve.unwrap();
        let nodes_map = resolve
            .nodes
            .into_iter()
            .map(|n| (n.id.to_owned(), n))
            .collect::<HashMap<_, _>>();
        let lock_file = cargo_metadata.workspace_root.join("Cargo.lock");
        let lock_content =
            Lockfile::load(&lock_file).unwrap_or_exit_ctx("failed to load Cargo.lock");
        let checksums_map = lock_content
            .packages
            .into_iter()
            .filter(|p| p.checksum.is_some())
            .map(|p| (format!("{}-{}", p.name, p.version), p.checksum.unwrap()))
            .collect::<HashMap<_, _>>();
        let repo_config = RepoConfig::load();
        Self {
            root,
            nodes_map,
            packages_map,
            checksums_map,
            workspace_root: cargo_metadata.workspace_root.clone(),
            no_merge: false,
            separate: false,
            repo_config,
        }
    }
}
