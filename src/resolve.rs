use std::collections::HashMap;
use std::path::PathBuf;

use cargo_metadata::{PackageId, camino::Utf8PathBuf};
use daggy::{Dag, NodeIndex, Walker};
use serde::{Deserialize, Serialize};

use crate::cache::{BuckalHash, Fingerprint};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    FirstParty { relative_path: String },
    ThirdParty,
}

/// A single dependency edge with platform/kind metadata.
///
/// This mirrors the relevant parts of `cargo_metadata::NodeDep` but uses
/// plain serializable types so it can be included in the cache fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuckalDep {
    /// The PackageId of the dependency.
    pub pkg: PackageId,
    /// The name of the dependency (may differ from package name if renamed).
    pub name: String,
    /// Dependency kind + optional platform constraint for each edge.
    pub dep_kinds: Vec<BuckalDepKind>,
}

/// Serializable representation of a dependency kind with an optional platform target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuckalDepKind {
    /// "normal", "dev", or "build"
    pub kind: String,
    /// Platform constraint string (e.g. `cfg(target_os = "linux")`), if any.
    pub target: Option<String>,
}

/// Serializable representation of a Cargo target (lib/bin/test/build-script).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuckalTarget {
    pub name: String,
    /// e.g. `["lib"]`, `["bin"]`, `["proc-macro"]`, `["custom-build"]`
    pub kind: Vec<String>,
    pub src_path: Utf8PathBuf,
    /// Whether doc-tests are enabled (used by lib targets).
    pub doctest: bool,
    /// Whether tests are enabled for this target.
    pub test: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuckalNode {
    pub package_id: PackageId,
    pub name: String,
    pub version: String,
    pub features: Vec<String>,
    pub kind: NodeKind,
    pub edition: String,
    /// Full dependency edges with kind/platform info (replaces the old `dep_ids`).
    pub deps: Vec<BuckalDep>,
    // -- Fields from Package --
    pub manifest_path: Utf8PathBuf,
    pub targets: Vec<BuckalTarget>,
    /// `None` for local (first-party) packages; `Some(repr)` for registry/git sources.
    pub source: Option<String>,
    /// The `links` manifest key, if any.
    pub links: Option<String>,
    /// Cargo.lock checksum for this package, if available.
    pub checksum: Option<String>,
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
    /// O(1) lookup of a node by its `PackageId`.
    pub fn get(&self, pkg_id: &PackageId) -> Option<&BuckalNode> {
        self.index_map.get(pkg_id).map(|&idx| &self.dag[idx])
    }

    /// Build a DAG from raw cargo metadata maps. `root_path` is used to compute
    /// relative paths for first-party packages (typically the buck2 root or workspace root).
    pub fn from_metadata(
        nodes_map: &HashMap<PackageId, cargo_metadata::Node>,
        packages_map: &HashMap<PackageId, cargo_metadata::Package>,
        checksums_map: &HashMap<String, String>,
        root_path: &std::path::Path,
    ) -> Self {
        let mut dag = Dag::<BuckalNode, (), u32>::new();
        let mut index_map = HashMap::new();

        // Create nodes
        for (pkg_id, node) in nodes_map {
            let package = packages_map.get(pkg_id).expect("package not found");

            let kind = if package.source.is_none() {
                // Local path dep — only first-party if under root_path
                let manifest_path = PathBuf::from(package.manifest_path.as_str());
                let manifest_dir = manifest_path
                    .parent()
                    .expect("manifest_path should have a parent");
                if let Ok(relative) = manifest_dir.strip_prefix(root_path) {
                    let relative_path = relative.to_string_lossy().replace('\\', "/");
                    NodeKind::FirstParty { relative_path }
                } else {
                    // Path dep outside workspace root — treat as third-party
                    NodeKind::ThirdParty
                }
            } else {
                NodeKind::ThirdParty
            };

            let deps: Vec<BuckalDep> = node
                .deps
                .iter()
                .map(|d| BuckalDep {
                    pkg: d.pkg.clone(),
                    name: d.name.clone(),
                    dep_kinds: d
                        .dep_kinds
                        .iter()
                        .map(|dk| BuckalDepKind {
                            kind: format!("{:?}", dk.kind),
                            target: dk.target.as_ref().map(|t| format!("{}", t)),
                        })
                        .collect(),
                })
                .collect();

            let targets: Vec<BuckalTarget> = package
                .targets
                .iter()
                .map(|t| BuckalTarget {
                    name: t.name.clone(),
                    kind: t.kind.iter().map(|k| format!("{}", k)).collect(),
                    src_path: t.src_path.clone(),
                    doctest: t.doctest,
                    test: t.test,
                })
                .collect();

            let checksum_key = format!("{}-{}", package.name, package.version);

            let buckal_node = BuckalNode {
                package_id: pkg_id.clone(),
                name: package.name.to_string(),
                version: package.version.to_string(),
                features: node.features.iter().map(|f| f.to_string()).collect(),
                kind,
                edition: package.edition.to_string(),
                deps,
                manifest_path: package.manifest_path.clone(),
                targets,
                source: package.source.as_ref().map(|s| s.repr.clone()),
                links: package.links.clone(),
                checksum: checksums_map.get(&checksum_key).cloned(),
            };

            let idx = dag.add_node(buckal_node);
            index_map.insert(pkg_id.clone(), idx);
        }

        // Create edges
        for (pkg_id, node) in nodes_map {
            if let Some(&parent_idx) = index_map.get(pkg_id) {
                for dep in &node.deps {
                    if let Some(&child_idx) = index_map.get(&dep.pkg)
                        && dag.add_edge(parent_idx, child_idx, ()).is_err()
                    {
                        log::warn!(
                            "Detected cycle when adding edge from {} to {:?} — skipping",
                            pkg_id.repr,
                            dep.pkg.repr
                        );
                    }
                }
            }
        }

        Self { dag, index_map }
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
            .find(|node| node.name == name && version.is_none_or(|v| node.version == v))
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
            repr: format!(
                "registry+https://github.com/rust-lang/crates.io-index#{}@1.0.0",
                name
            ),
        }
    }

    fn make_pkg_id_versioned(name: &str, version: &str) -> PackageId {
        PackageId {
            repr: format!(
                "registry+https://github.com/rust-lang/crates.io-index#{}@{}",
                name, version
            ),
        }
    }

    fn make_node(name: &str, version: &str, dep_pkg_ids: Vec<PackageId>) -> BuckalNode {
        let deps = dep_pkg_ids
            .into_iter()
            .map(|pkg| BuckalDep {
                name: pkg
                    .repr
                    .rsplit('#')
                    .next()
                    .unwrap()
                    .split('@')
                    .next()
                    .unwrap()
                    .to_string(),
                pkg,
                dep_kinds: vec![BuckalDepKind {
                    kind: "Normal".to_string(),
                    target: None,
                }],
            })
            .collect();
        BuckalNode {
            package_id: make_pkg_id(name),
            name: name.to_string(),
            version: version.to_string(),
            features: vec![],
            kind: NodeKind::ThirdParty,
            edition: "2021".to_string(),
            deps,
            manifest_path: Utf8PathBuf::from(format!("/tmp/{}/Cargo.toml", name)),
            targets: vec![],
            source: Some("registry+https://github.com/rust-lang/crates.io-index".to_string()),
            links: None,
            checksum: None,
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
        let mut node = make_node("my-crate", "0.1.0", vec![]);
        node.package_id = make_pkg_id("my-crate");
        node.kind = NodeKind::FirstParty {
            relative_path: "crates/my-crate".to_string(),
        };
        node.source = None;

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

    /// Diamond dependency with version conflict:
    ///
    ///     root
    ///    /    \
    /// dep_a  dep_b
    ///    \    /
    ///   common  (v1.0.0 via dep_a, v2.0.0 via dep_b)
    ///
    /// Both versions of `common` must exist as separate nodes in the DAG.
    #[test]
    fn test_diamond_dependency_version_conflict() {
        let mut dag = Dag::<BuckalNode, (), u32>::new();
        let mut index_map = HashMap::new();

        let common_v1_id = make_pkg_id_versioned("common", "1.0.0");
        let common_v2_id = make_pkg_id_versioned("common", "2.0.0");

        let root = {
            let mut n = make_node(
                "root",
                "0.1.0",
                vec![make_pkg_id("dep-a"), make_pkg_id("dep-b")],
            );
            n.kind = NodeKind::FirstParty {
                relative_path: "".to_string(),
            };
            n.source = None;
            n
        };

        let dep_a = make_node("dep-a", "1.0.0", vec![common_v1_id.clone()]);
        let dep_b = make_node("dep-b", "1.0.0", vec![common_v2_id.clone()]);

        let common_v1 = {
            let mut n = make_node("common", "1.0.0", vec![]);
            n.package_id = common_v1_id.clone();
            n
        };

        let common_v2 = {
            let mut n = make_node("common", "2.0.0", vec![]);
            n.package_id = common_v2_id.clone();
            n
        };

        let idx_root = dag.add_node(root.clone());
        let idx_a = dag.add_node(dep_a.clone());
        let idx_b = dag.add_node(dep_b.clone());
        let idx_cv1 = dag.add_node(common_v1.clone());
        let idx_cv2 = dag.add_node(common_v2.clone());

        index_map.insert(root.package_id.clone(), idx_root);
        index_map.insert(dep_a.package_id.clone(), idx_a);
        index_map.insert(dep_b.package_id.clone(), idx_b);
        index_map.insert(common_v1_id.clone(), idx_cv1);
        index_map.insert(common_v2_id.clone(), idx_cv2);

        dag.add_edge(idx_root, idx_a, ()).unwrap();
        dag.add_edge(idx_root, idx_b, ()).unwrap();
        dag.add_edge(idx_a, idx_cv1, ()).unwrap();
        dag.add_edge(idx_b, idx_cv2, ()).unwrap();

        let resolve = BuckalResolve { dag, index_map };

        // Total: root + dep_a + dep_b + common@1.0 + common@2.0 = 5 nodes
        assert_eq!(resolve.nodes().count(), 5);

        // Both versions of common exist as separate nodes
        assert!(resolve.find_by_name("common", Some("1.0.0")).is_some());
        assert!(resolve.find_by_name("common", Some("2.0.0")).is_some());

        // find_by_name without version returns one (non-deterministic which)
        assert!(resolve.find_by_name("common", None).is_some());

        // Count nodes named "common" — should be exactly 2
        let common_nodes: Vec<&BuckalNode> =
            resolve.nodes().filter(|n| n.name == "common").collect();
        assert_eq!(
            common_nodes.len(),
            2,
            "expected 2 nodes named 'common', got {}",
            common_nodes.len()
        );

        // dep_a depends on common@1.0.0 only
        let a_deps = resolve.dependencies(&make_pkg_id("dep-a"));
        assert_eq!(a_deps.len(), 1);
        assert_eq!(a_deps[0].name, "common");
        assert_eq!(a_deps[0].version, "1.0.0");

        // dep_b depends on common@2.0.0 only
        let b_deps = resolve.dependencies(&make_pkg_id("dep-b"));
        assert_eq!(b_deps.len(), 1);
        assert_eq!(b_deps[0].name, "common");
        assert_eq!(b_deps[0].version, "2.0.0");

        // common@1.0.0 dependents should be [dep_a] only
        let cv1_dependents = resolve.dependents(&common_v1_id);
        assert_eq!(cv1_dependents.len(), 1);
        assert_eq!(cv1_dependents[0].name, "dep-a");

        // common@2.0.0 dependents should be [dep_b] only
        let cv2_dependents = resolve.dependents(&common_v2_id);
        assert_eq!(cv2_dependents.len(), 1);
        assert_eq!(cv2_dependents[0].name, "dep-b");

        // root depends on both dep_a and dep_b
        let root_deps = resolve.dependencies(&make_pkg_id("root"));
        assert_eq!(root_deps.len(), 2);
        let root_dep_names: Vec<&str> = root_deps.iter().map(|n| n.name.as_str()).collect();
        assert!(root_dep_names.contains(&"dep-a"));
        assert!(root_dep_names.contains(&"dep-b"));

        // Fingerprints of common@1.0.0 and common@2.0.0 must differ
        assert_ne!(
            common_v1.fingerprint(),
            common_v2.fingerprint(),
            "different versions should produce different fingerprints"
        );
    }

    /// Integration test: diamond dependency with version conflict using a real Cargo workspace.
    ///
    /// The fixture at `tests/fixtures/diamond-deps/` is a workspace with three crates:
    /// - `uses-itoa-old` depends on `itoa = "0.4"`
    /// - `uses-itoa-new` depends on `itoa = "1"`
    /// - `diamond-root` depends on both `uses-itoa-old` and `uses-itoa-new`
    ///
    /// This creates a true diamond dependency: `diamond-root` transitively pulls in
    /// two semver-incompatible versions of `itoa` through different intermediate crates.
    /// Cargo resolves both `itoa 0.4.x` and `itoa 1.x` since they are semver-incompatible,
    /// producing two separate nodes in the DAG for the same crate name.
    #[test]
    fn test_diamond_deps_version_conflict() {
        use cargo_metadata::MetadataCommand;

        let fixture_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/diamond-deps");
        assert!(
            fixture_dir.join("Cargo.toml").exists(),
            "fixture not found at {}",
            fixture_dir.display()
        );

        let manifest_path = fixture_dir.join("Cargo.toml");
        let metadata = MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .expect("cargo metadata failed");
        let packages_map: HashMap<PackageId, cargo_metadata::Package> = metadata
            .packages
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let resolve_meta = metadata.resolve.expect("no resolve in metadata");
        let nodes_map: HashMap<PackageId, cargo_metadata::Node> = resolve_meta
            .nodes
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect();
        let root_path = std::path::Path::new(metadata.workspace_root.as_str());
        let resolve =
            BuckalResolve::from_metadata(&nodes_map, &packages_map, &HashMap::new(), root_path);

        // Both workspace members should be FirstParty
        let old_crate = resolve
            .find_by_name("uses-itoa-old", None)
            .expect("uses-itoa-old not found");
        let new_crate = resolve
            .find_by_name("uses-itoa-new", None)
            .expect("uses-itoa-new not found");
        assert!(matches!(&old_crate.kind, NodeKind::FirstParty { .. }));
        assert!(matches!(&new_crate.kind, NodeKind::FirstParty { .. }));

        // Both itoa versions should exist as separate ThirdParty nodes
        let itoa_nodes: Vec<&BuckalNode> = resolve.nodes().filter(|n| n.name == "itoa").collect();
        assert_eq!(
            itoa_nodes.len(),
            2,
            "expected 2 itoa nodes (0.4.x and 1.x), got {}: {:?}",
            itoa_nodes.len(),
            itoa_nodes.iter().map(|n| &n.version).collect::<Vec<_>>()
        );

        // Verify the two itoa nodes have different major versions
        let itoa_versions: Vec<&str> = itoa_nodes.iter().map(|n| n.version.as_str()).collect();
        assert!(
            itoa_versions.iter().any(|v: &&str| v.starts_with("0.4")),
            "expected an itoa 0.4.x, got {:?}",
            itoa_versions
        );
        assert!(
            itoa_versions.iter().any(|v: &&str| v.starts_with("1.")),
            "expected an itoa 1.x, got {:?}",
            itoa_versions
        );

        // Both itoa nodes should be ThirdParty
        for node in &itoa_nodes {
            assert!(
                matches!(&node.kind, NodeKind::ThirdParty),
                "itoa {} should be ThirdParty",
                node.version
            );
        }

        // uses-itoa-old should depend on itoa 0.4.x
        let old_deps = resolve.dependencies(&old_crate.package_id);
        let old_itoa = old_deps
            .iter()
            .find(|n| n.name == "itoa")
            .expect("uses-itoa-old should depend on itoa");
        assert!(
            old_itoa.version.starts_with("0.4"),
            "uses-itoa-old should depend on itoa 0.4.x, got {}",
            old_itoa.version
        );

        // uses-itoa-new should depend on itoa 1.x
        let new_deps = resolve.dependencies(&new_crate.package_id);
        let new_itoa = new_deps
            .iter()
            .find(|n| n.name == "itoa")
            .expect("uses-itoa-new should depend on itoa");
        assert!(
            new_itoa.version.starts_with("1."),
            "uses-itoa-new should depend on itoa 1.x, got {}",
            new_itoa.version
        );

        // itoa 0.4.x dependents should include uses-itoa-old but not uses-itoa-new
        let itoa_old_node = itoa_nodes
            .iter()
            .find(|n| n.version.starts_with("0.4"))
            .unwrap();
        let old_dependents = resolve.dependents(&itoa_old_node.package_id);
        let old_dep_names: Vec<&str> = old_dependents.iter().map(|n| n.name.as_str()).collect();
        assert!(
            old_dep_names.contains(&"uses-itoa-old"),
            "itoa 0.4.x dependents should include uses-itoa-old, got {:?}",
            old_dep_names
        );
        assert!(
            !old_dep_names.contains(&"uses-itoa-new"),
            "itoa 0.4.x dependents should NOT include uses-itoa-new"
        );

        // itoa 1.x dependents should include uses-itoa-new but not uses-itoa-old
        let itoa_new_node = itoa_nodes
            .iter()
            .find(|n| n.version.starts_with("1."))
            .unwrap();
        let new_dependents = resolve.dependents(&itoa_new_node.package_id);
        let new_dep_names: Vec<&str> = new_dependents.iter().map(|n| n.name.as_str()).collect();
        assert!(
            new_dep_names.contains(&"uses-itoa-new"),
            "itoa 1.x dependents should include uses-itoa-new, got {:?}",
            new_dep_names
        );
        assert!(
            !new_dep_names.contains(&"uses-itoa-old"),
            "itoa 1.x dependents should NOT include uses-itoa-old"
        );

        // find_by_name with version filtering should find the correct node
        let found_old = resolve
            .find_by_name("itoa", Some(&itoa_old_node.version))
            .unwrap();
        assert!(found_old.version.starts_with("0.4"));
        let found_new = resolve
            .find_by_name("itoa", Some(&itoa_new_node.version))
            .unwrap();
        assert!(found_new.version.starts_with("1."));

        // Fingerprints of the two itoa versions must differ
        assert_ne!(
            itoa_old_node.fingerprint(),
            itoa_new_node.fingerprint(),
            "different itoa versions should have different fingerprints"
        );

        // diamond-root should be a FirstParty node
        let diamond_root = resolve
            .find_by_name("diamond-root", None)
            .expect("diamond-root not found");
        assert!(matches!(&diamond_root.kind, NodeKind::FirstParty { .. }));

        // diamond-root should depend on both uses-itoa-old and uses-itoa-new
        let root_deps = resolve.dependencies(&diamond_root.package_id);
        let root_dep_names: Vec<&str> = root_deps.iter().map(|n| n.name.as_str()).collect();
        assert!(
            root_dep_names.contains(&"uses-itoa-old"),
            "diamond-root should depend on uses-itoa-old, got {:?}",
            root_dep_names
        );
        assert!(
            root_dep_names.contains(&"uses-itoa-new"),
            "diamond-root should depend on uses-itoa-new, got {:?}",
            root_dep_names
        );

        // Traversing the DAG from diamond-root should reach both itoa versions transitively
        let mut transitive_itoa_versions: Vec<String> = Vec::new();
        for dep in &root_deps {
            for transitive in resolve.dependencies(&dep.package_id) {
                if transitive.name == "itoa" {
                    transitive_itoa_versions.push(transitive.version.clone());
                }
            }
        }
        assert!(
            transitive_itoa_versions
                .iter()
                .any(|v| v.starts_with("0.4")),
            "diamond-root should transitively reach itoa 0.4.x, got {:?}",
            transitive_itoa_versions
        );
        assert!(
            transitive_itoa_versions.iter().any(|v| v.starts_with("1.")),
            "diamond-root should transitively reach itoa 1.x, got {:?}",
            transitive_itoa_versions
        );

        // Cache construction should work
        let ws_root = cargo_metadata::camino::Utf8PathBuf::from(fixture_dir.to_str().unwrap());
        let cache = crate::cache::BuckalCache::from_resolve(&resolve, &ws_root);
        let cache_str = toml::to_string_pretty(&cache).unwrap();
        assert!(
            cache_str.contains("fingerprints"),
            "cache should contain fingerprints section"
        );
    }

    /// Integration test: diamond dependency with semver-compatible versions.
    ///
    /// The fixture at `tests/fixtures/diamond-deps-compat/` is a workspace with three crates:
    /// - `uses-itoa-loose` depends on `itoa = "1.0"` (>=1.0.0, <2.0.0)
    /// - `uses-itoa-pinned` depends on `itoa = "1.0.5"` (>=1.0.5, <2.0.0)
    /// - `compat-root` depends on both
    ///
    /// Since both constraints are semver-compatible, Cargo unifies them into a single
    /// `itoa` version (>=1.0.5) — producing exactly one node in the DAG, unlike the
    /// incompatible diamond which produces two.
    #[test]
    fn test_diamond_deps_semver_compatible() {
        use cargo_metadata::MetadataCommand;

        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/diamond-deps-compat");
        assert!(
            fixture_dir.join("Cargo.toml").exists(),
            "fixture not found at {}",
            fixture_dir.display()
        );

        let manifest_path = fixture_dir.join("Cargo.toml");
        let metadata = MetadataCommand::new()
            .manifest_path(&manifest_path)
            .exec()
            .expect("cargo metadata failed");
        let packages_map: HashMap<PackageId, cargo_metadata::Package> = metadata
            .packages
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let resolve_meta = metadata.resolve.expect("no resolve in metadata");
        let nodes_map: HashMap<PackageId, cargo_metadata::Node> = resolve_meta
            .nodes
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect();
        let root_path = std::path::Path::new(metadata.workspace_root.as_str());
        let resolve =
            BuckalResolve::from_metadata(&nodes_map, &packages_map, &HashMap::new(), root_path);

        // All three workspace members should be FirstParty
        let loose = resolve
            .find_by_name("uses-itoa-loose", None)
            .expect("uses-itoa-loose not found");
        let pinned = resolve
            .find_by_name("uses-itoa-pinned", None)
            .expect("uses-itoa-pinned not found");
        let root = resolve
            .find_by_name("compat-root", None)
            .expect("compat-root not found");
        assert!(matches!(&loose.kind, NodeKind::FirstParty { .. }));
        assert!(matches!(&pinned.kind, NodeKind::FirstParty { .. }));
        assert!(matches!(&root.kind, NodeKind::FirstParty { .. }));

        // Cargo should unify both constraints into exactly ONE itoa node
        let itoa_nodes: Vec<&BuckalNode> = resolve.nodes().filter(|n| n.name == "itoa").collect();
        assert_eq!(
            itoa_nodes.len(),
            1,
            "semver-compatible constraints should unify to 1 itoa node, got {}: {:?}",
            itoa_nodes.len(),
            itoa_nodes.iter().map(|n| &n.version).collect::<Vec<_>>()
        );

        let itoa = itoa_nodes[0];
        assert!(
            itoa.version.starts_with("1."),
            "unified itoa should be 1.x, got {}",
            itoa.version
        );
        assert!(matches!(&itoa.kind, NodeKind::ThirdParty));

        // Both intermediate crates should depend on the SAME itoa node
        let loose_deps = resolve.dependencies(&loose.package_id);
        let pinned_deps = resolve.dependencies(&pinned.package_id);
        let loose_itoa = loose_deps
            .iter()
            .find(|n| n.name == "itoa")
            .expect("uses-itoa-loose should depend on itoa");
        let pinned_itoa = pinned_deps
            .iter()
            .find(|n| n.name == "itoa")
            .expect("uses-itoa-pinned should depend on itoa");
        assert_eq!(
            loose_itoa.package_id, pinned_itoa.package_id,
            "both crates should resolve to the same itoa: {} vs {}",
            loose_itoa.version, pinned_itoa.version
        );

        // The single itoa node should have both crates as dependents
        let itoa_dependents = resolve.dependents(&itoa.package_id);
        let dep_names: Vec<&str> = itoa_dependents.iter().map(|n| n.name.as_str()).collect();
        assert!(
            dep_names.contains(&"uses-itoa-loose"),
            "itoa dependents should include uses-itoa-loose, got {:?}",
            dep_names
        );
        assert!(
            dep_names.contains(&"uses-itoa-pinned"),
            "itoa dependents should include uses-itoa-pinned, got {:?}",
            dep_names
        );

        // compat-root should depend on both intermediate crates
        let root_deps = resolve.dependencies(&root.package_id);
        let root_dep_names: Vec<&str> = root_deps.iter().map(|n| n.name.as_str()).collect();
        assert!(root_dep_names.contains(&"uses-itoa-loose"));
        assert!(root_dep_names.contains(&"uses-itoa-pinned"));

        // Traversing from compat-root through both paths should reach the same itoa
        let mut transitive_itoa_ids: Vec<&PackageId> = Vec::new();
        for dep in &root_deps {
            for transitive in resolve.dependencies(&dep.package_id) {
                if transitive.name == "itoa" {
                    transitive_itoa_ids.push(&transitive.package_id);
                }
            }
        }
        assert_eq!(
            transitive_itoa_ids.len(),
            2,
            "should reach itoa through 2 paths, got {}",
            transitive_itoa_ids.len()
        );
        assert_eq!(
            transitive_itoa_ids[0], transitive_itoa_ids[1],
            "both paths should reach the same itoa node"
        );
    }
}
