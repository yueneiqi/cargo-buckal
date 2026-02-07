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
        let buck2_root_path = PathBuf::from(buck2_root.as_str());

        let mut dag = Dag::<BuckalNode, (), u32>::new();
        let mut index_map = HashMap::new();

        // Create nodes
        for (pkg_id, node) in &ctx.nodes_map {
            let package = ctx.packages_map.get(pkg_id).expect("package not found");

            let kind = if package.source.is_none() {
                // First-party (workspace member)
                let manifest_path = PathBuf::from(package.manifest_path.as_str());
                let manifest_dir = manifest_path
                    .parent()
                    .expect("manifest_path should have a parent");
                let relative_path = manifest_dir
                    .strip_prefix(&buck2_root_path)
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
        for (pkg_id, node) in &ctx.nodes_map {
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
}
