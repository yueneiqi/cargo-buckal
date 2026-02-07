use std::collections::HashMap;
use std::path::PathBuf;

use cargo_metadata::PackageId;
use daggy::{Dag, NodeIndex, WouldCycle, Walker};
use serde::{Deserialize, Serialize};

use crate::cache::{BuckalHash, Fingerprint};
use crate::context::BuckalContext;
use crate::utils::get_buck2_root;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    FirstParty { relative_path: String },
    ThirdParty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuckalNode {
    pub package_id: PackageId,
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
    pub kind: NodeKind,
    pub edition: String,
    pub dep_ids: Vec<PackageId>,
}

impl BuckalHash for BuckalNode {
    fn fingerprint(&self) -> Fingerprint {
        let encoded = bincode::serde::encode_to_vec(self, bincode::config::standard())
            .expect("Serialization failed");
        Fingerprint::new(blake3::hash(&encoded).into())
    }
}

pub struct BuckalResolve {
    pub dag: Dag<BuckalNode, (), u32>,
    pub index_map: HashMap<PackageId, NodeIndex<u32>>,
}

impl BuckalResolve {
    pub fn from_context(ctx: &BuckalContext) -> Self {
        let buck2_root = get_buck2_root().expect("failed to get buck2 root");
        Self::from_metadata(&ctx.nodes_map, &ctx.packages_map, buck2_root.as_std_path())
    }

    /// Build a DAG from raw cargo metadata maps. `root_path` is used to compute
    /// relative paths for first-party packages (typically the buck2 root or workspace root).
    pub fn from_metadata(
        nodes_map: &HashMap<PackageId, cargo_metadata::Node>,
        packages_map: &HashMap<PackageId, cargo_metadata::Package>,
        root_path: &std::path::Path,
    ) -> Self {
        let mut dag = Dag::<BuckalNode, (), u32>::new();
        let mut index_map = HashMap::new();

        // Create nodes
        for (pkg_id, node) in nodes_map {
            let package = packages_map.get(pkg_id).expect("package not found");

            let kind = if package.source.is_none() {
                // First-party (workspace member)
                let manifest_path = PathBuf::from(package.manifest_path.as_str());
                let manifest_dir = manifest_path
                    .parent()
                    .expect("manifest_path should have a parent");
                let relative_path = manifest_dir
                    .strip_prefix(root_path)
                    .unwrap_or(manifest_dir)
                    .to_string_lossy()
                    .replace('\\', "/");
                NodeKind::FirstParty { relative_path }
            } else {
                NodeKind::ThirdParty
            };

            let dep_ids: Vec<PackageId> = node.deps.iter().map(|d| d.pkg.clone()).collect();

            let buckal_node = BuckalNode {
                package_id: pkg_id.clone(),
                name: package.name.to_string(),
                version: package.version.to_string(),
                features: node.features.iter().map(|f| f.to_string()).collect(),
                kind,
                edition: package.edition.to_string(),
                dep_ids,
            };

            let idx = dag.add_node(buckal_node);
            index_map.insert(pkg_id.clone(), idx);
        }

        // Create edges
        for (pkg_id, node) in nodes_map {
            if let Some(&parent_idx) = index_map.get(pkg_id) {
                for dep in &node.deps {
                    if let Some(&child_idx) = index_map.get(&dep.pkg) {
                        // Ignore WouldCycle errors - shouldn't happen for valid Cargo graphs
                        let _: Result<_, WouldCycle<()>> =
                            dag.add_edge(parent_idx, child_idx, ());
                    }
                }
            }
        }

        Self { dag, index_map }
    }

    pub fn get_node(&self, pkg_id: &PackageId) -> Option<&BuckalNode> {
        self.index_map
            .get(pkg_id)
            .map(|&idx| &self.dag[idx])
    }

    pub fn dependents(&self, pkg_id: &PackageId) -> Vec<&BuckalNode> {
        let Some(&idx) = self.index_map.get(pkg_id) else {
            return Vec::new();
        };
        self.dag
            .parents(idx)
            .iter(&self.dag)
            .map(|(_edge, node_idx)| &self.dag[node_idx])
            .collect()
    }

    pub fn dependencies(&self, pkg_id: &PackageId) -> Vec<&BuckalNode> {
        let Some(&idx) = self.index_map.get(pkg_id) else {
            return Vec::new();
        };
        self.dag
            .children(idx)
            .iter(&self.dag)
            .map(|(_edge, node_idx)| &self.dag[node_idx])
            .collect()
    }

    pub fn find_by_name(&self, name: &str, version: Option<&str>) -> Option<&BuckalNode> {
        self.dag
            .raw_nodes()
            .iter()
            .map(|n| &n.weight)
            .find(|node| {
                node.name == name
                    && version.map_or(true, |v| node.version == v)
            })
    }

    pub fn nodes(&self) -> impl Iterator<Item = &BuckalNode> {
        self.dag.raw_nodes().iter().map(|n| &n.weight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pkg_id(name: &str) -> PackageId {
        PackageId {
            repr: format!("registry+https://github.com/rust-lang/crates.io-index#{}@1.0.0", name),
        }
    }

    fn make_node(name: &str, version: &str, dep_ids: Vec<PackageId>) -> BuckalNode {
        BuckalNode {
            package_id: make_pkg_id(name),
            name: name.to_string(),
            version: version.to_string(),
            features: vec![],
            kind: NodeKind::ThirdParty,
            edition: "2021".to_string(),
            dep_ids,
        }
    }

    #[test]
    fn test_three_node_chain() {
        let mut dag = Dag::<BuckalNode, (), u32>::new();
        let mut index_map = HashMap::new();

        let node_a = make_node("a", "1.0.0", vec![make_pkg_id("b")]);
        let node_b = make_node("b", "1.0.0", vec![make_pkg_id("c")]);
        let node_c = make_node("c", "1.0.0", vec![]);

        let idx_a = dag.add_node(node_a.clone());
        let idx_b = dag.add_node(node_b.clone());
        let idx_c = dag.add_node(node_c.clone());

        index_map.insert(node_a.package_id.clone(), idx_a);
        index_map.insert(node_b.package_id.clone(), idx_b);
        index_map.insert(node_c.package_id.clone(), idx_c);

        dag.add_edge(idx_a, idx_b, ()).unwrap();
        dag.add_edge(idx_b, idx_c, ()).unwrap();

        let resolve = BuckalResolve { dag, index_map };

        // B's dependents should be [A]
        let b_dependents = resolve.dependents(&make_pkg_id("b"));
        assert_eq!(b_dependents.len(), 1);
        assert_eq!(b_dependents[0].name, "a");

        // A's dependencies should be [B]
        let a_deps = resolve.dependencies(&make_pkg_id("a"));
        assert_eq!(a_deps.len(), 1);
        assert_eq!(a_deps[0].name, "b");

        // C has no dependents besides B
        let c_dependents = resolve.dependents(&make_pkg_id("c"));
        assert_eq!(c_dependents.len(), 1);
        assert_eq!(c_dependents[0].name, "b");
    }

    #[test]
    fn test_first_party_relative_path() {
        let node = BuckalNode {
            package_id: make_pkg_id("my-crate"),
            name: "my-crate".to_string(),
            version: "0.1.0".to_string(),
            features: vec![],
            kind: NodeKind::FirstParty {
                relative_path: "crates/my-crate".to_string(),
            },
            edition: "2021".to_string(),
            dep_ids: vec![],
        };

        match &node.kind {
            NodeKind::FirstParty { relative_path } => {
                assert_eq!(relative_path, "crates/my-crate");
            }
            _ => panic!("expected FirstParty"),
        }
    }

    #[test]
    fn test_find_by_name() {
        let mut dag = Dag::<BuckalNode, (), u32>::new();
        let mut index_map = HashMap::new();

        let node_a = make_node("serde", "1.0.0", vec![]);
        let node_b = make_node("tokio", "1.0.0", vec![]);

        let idx_a = dag.add_node(node_a.clone());
        let idx_b = dag.add_node(node_b.clone());

        index_map.insert(node_a.package_id.clone(), idx_a);
        index_map.insert(node_b.package_id.clone(), idx_b);

        let resolve = BuckalResolve { dag, index_map };

        assert!(resolve.find_by_name("serde", None).is_some());
        assert!(resolve.find_by_name("serde", Some("1.0.0")).is_some());
        assert!(resolve.find_by_name("serde", Some("2.0.0")).is_none());
        assert!(resolve.find_by_name("nonexistent", None).is_none());
    }

    #[test]
    fn test_fingerprint_stability_and_sensitivity() {
        let node1 = make_node("foo", "1.0.0", vec![]);
        let node2 = make_node("foo", "1.0.0", vec![]);
        let node3 = make_node("foo", "1.1.0", vec![]);

        // Same data -> same fingerprint
        assert_eq!(node1.fingerprint(), node2.fingerprint());

        // Different version -> different fingerprint
        assert_ne!(node1.fingerprint(), node3.fingerprint());
    }

    /// Helper: build a BuckalResolve from cargo metadata at `manifest_dir`.
    fn resolve_from_manifest(manifest_dir: &str) -> Option<BuckalResolve> {
        use cargo_metadata::MetadataCommand;
        let manifest_path = std::path::Path::new(manifest_dir).join("Cargo.toml");
        if !manifest_path.exists() {
            return None;
        }
        let metadata = MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .ok()?;
        let packages_map: HashMap<PackageId, cargo_metadata::Package> = metadata
            .packages
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let resolve = metadata.resolve?;
        let nodes_map: HashMap<PackageId, cargo_metadata::Node> = resolve
            .nodes
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect();
        let root_path = std::path::Path::new(metadata.workspace_root.as_str());
        Some(BuckalResolve::from_metadata(&nodes_map, &packages_map, root_path))
    }

    #[test]
    fn test_first_party_demo_dag() {
        let Some(resolve) = resolve_from_manifest("/tmp/buckal-test/first-party-demo") else {
            eprintln!("skipping: first-party-demo not cloned at /tmp/buckal-test/first-party-demo");
            return;
        };

        // Should contain the 3 first-party crates
        assert!(resolve.find_by_name("demo-root", None).is_some(), "demo-root not found");
        assert!(resolve.find_by_name("demo-lib", None).is_some(), "demo-lib not found");
        assert!(resolve.find_by_name("demo-util", None).is_some(), "demo-util not found");

        // All 3 first-party crates should be FirstParty
        for name in &["demo-root", "demo-lib", "demo-util"] {
            let node = resolve.find_by_name(name, None).unwrap();
            assert!(
                matches!(&node.kind, NodeKind::FirstParty { .. }),
                "{} should be FirstParty, got {:?}",
                name,
                node.kind
            );
        }

        // demo-root relative_path should be "" (it's at the workspace root)
        let root_node = resolve.find_by_name("demo-root", None).unwrap();
        match &root_node.kind {
            NodeKind::FirstParty { relative_path } => {
                assert_eq!(relative_path, "", "demo-root should have empty relative_path");
            }
            _ => panic!("expected FirstParty"),
        }

        // demo-lib relative path should be "crates/demo-lib"
        let lib_node = resolve.find_by_name("demo-lib", None).unwrap();
        match &lib_node.kind {
            NodeKind::FirstParty { relative_path } => {
                assert_eq!(relative_path, "crates/demo-lib");
            }
            _ => panic!("expected FirstParty"),
        }

        // demo-util relative path should be "crates/demo-util"
        let util_node = resolve.find_by_name("demo-util", None).unwrap();
        match &util_node.kind {
            NodeKind::FirstParty { relative_path } => {
                assert_eq!(relative_path, "crates/demo-util");
            }
            _ => panic!("expected FirstParty"),
        }

        // serde should be ThirdParty
        let serde_node = resolve.find_by_name("serde", None).unwrap();
        assert!(
            matches!(&serde_node.kind, NodeKind::ThirdParty),
            "serde should be ThirdParty"
        );

        // demo-lib depends on serde, so serde's dependents should include demo-lib
        let serde_dependents = resolve.dependents(&serde_node.package_id);
        let serde_dependent_names: Vec<&str> = serde_dependents.iter().map(|n| n.name.as_str()).collect();
        assert!(
            serde_dependent_names.contains(&"demo-lib"),
            "serde dependents should include demo-lib, got {:?}",
            serde_dependent_names
        );

        // demo-lib's dependencies should include demo-util and serde
        let lib_deps = resolve.dependencies(&lib_node.package_id);
        let lib_dep_names: Vec<&str> = lib_deps.iter().map(|n| n.name.as_str()).collect();
        assert!(lib_dep_names.contains(&"demo-util"), "demo-lib should depend on demo-util, got {:?}", lib_dep_names);
        assert!(lib_dep_names.contains(&"serde"), "demo-lib should depend on serde, got {:?}", lib_dep_names);

        // Total node count should include all transitive deps
        let total_nodes = resolve.nodes().count();
        assert!(total_nodes >= 5, "expected at least 5 nodes (3 first-party + serde + serde_derive), got {}", total_nodes);

        // Cache construction should work
        let cache = crate::cache::BuckalCache::from_resolve(
            &resolve,
            &cargo_metadata::camino::Utf8PathBuf::from("/tmp/buckal-test/first-party-demo"),
        );
        // Verify cache has entries for all nodes
        let cache_str = toml::to_string_pretty(&cache).unwrap();
        assert!(cache_str.contains("fingerprints"), "cache should contain fingerprints section");
    }

    #[test]
    fn test_monorepo_demo_dag() {
        let Some(resolve) = resolve_from_manifest("/tmp/buckal-test/monorepo-demo/project") else {
            eprintln!("skipping: monorepo-demo not cloned at /tmp/buckal-test/monorepo-demo");
            return;
        };

        // Should contain the 2 workspace members (virtual workspace - no root package)
        assert!(resolve.find_by_name("sub-lib", None).is_some(), "sub-lib not found");
        assert!(resolve.find_by_name("sub-app", None).is_some(), "sub-app not found");

        // Both should be FirstParty
        let sub_lib = resolve.find_by_name("sub-lib", None).unwrap();
        let sub_app = resolve.find_by_name("sub-app", None).unwrap();
        assert!(matches!(&sub_lib.kind, NodeKind::FirstParty { .. }), "sub-lib should be FirstParty");
        assert!(matches!(&sub_app.kind, NodeKind::FirstParty { .. }), "sub-app should be FirstParty");

        // Verify relative paths
        match &sub_lib.kind {
            NodeKind::FirstParty { relative_path } => {
                assert_eq!(relative_path, "sub-lib");
            }
            _ => panic!("expected FirstParty"),
        }
        match &sub_app.kind {
            NodeKind::FirstParty { relative_path } => {
                assert_eq!(relative_path, "sub-app");
            }
            _ => panic!("expected FirstParty"),
        }

        // sub-app depends on sub-lib
        let app_deps = resolve.dependencies(&sub_app.package_id);
        let app_dep_names: Vec<&str> = app_deps.iter().map(|n| n.name.as_str()).collect();
        assert!(
            app_dep_names.contains(&"sub-lib"),
            "sub-app should depend on sub-lib, got {:?}",
            app_dep_names
        );

        // sub-lib's dependents should include sub-app
        let lib_dependents = resolve.dependents(&sub_lib.package_id);
        let lib_dependent_names: Vec<&str> = lib_dependents.iter().map(|n| n.name.as_str()).collect();
        assert!(
            lib_dependent_names.contains(&"sub-app"),
            "sub-lib dependents should include sub-app, got {:?}",
            lib_dependent_names
        );

        // serde should be ThirdParty and present
        let serde_node = resolve.find_by_name("serde", None).unwrap();
        assert!(matches!(&serde_node.kind, NodeKind::ThirdParty));

        // sub-lib depends on serde (workspace dep)
        let lib_deps = resolve.dependencies(&sub_lib.package_id);
        let lib_dep_names: Vec<&str> = lib_deps.iter().map(|n| n.name.as_str()).collect();
        assert!(
            lib_dep_names.contains(&"serde"),
            "sub-lib should depend on serde, got {:?}",
            lib_dep_names
        );

        // Cache construction should work
        let cache = crate::cache::BuckalCache::from_resolve(
            &resolve,
            &cargo_metadata::camino::Utf8PathBuf::from("/tmp/buckal-test/monorepo-demo/project"),
        );
        let cache_str = toml::to_string_pretty(&cache).unwrap();
        assert!(cache_str.contains("fingerprints"), "cache should contain fingerprints section");
    }
}
