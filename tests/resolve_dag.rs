use std::collections::HashMap;

use cargo_buckal::cache::BuckalCache;
use cargo_buckal::resolve::{BuckalResolve, NodeKind};
use cargo_metadata::{MetadataCommand, PackageId};

/// Helper: build a BuckalResolve from cargo metadata at `manifest_dir`.
fn resolve_from_manifest(manifest_dir: &str) -> BuckalResolve {
    let manifest_path = std::path::Path::new(manifest_dir).join("Cargo.toml");
    let metadata = MetadataCommand::new()
        .manifest_path(&manifest_path)
        .exec()
        .expect("cargo metadata failed");
    let packages_map: HashMap<PackageId, cargo_metadata::Package> = metadata
        .packages
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();
    let resolve = metadata.resolve.expect("no resolve in metadata");
    let nodes_map: HashMap<PackageId, cargo_metadata::Node> = resolve
        .nodes
        .into_iter()
        .map(|n| (n.id.clone(), n))
        .collect();
    let root_path = std::path::Path::new(metadata.workspace_root.as_str());
    BuckalResolve::from_metadata(&nodes_map, &packages_map, &HashMap::new(), root_path)
}

#[test]
#[ignore]
fn test_dag_first_party_demo() {
    let resolve = resolve_from_manifest("/tmp/buckal-test/first-party-demo");

    // Should contain the 3 first-party crates
    assert!(
        resolve.find_by_name("demo-root", None).is_some(),
        "demo-root not found"
    );
    assert!(
        resolve.find_by_name("demo-lib", None).is_some(),
        "demo-lib not found"
    );
    assert!(
        resolve.find_by_name("demo-util", None).is_some(),
        "demo-util not found"
    );

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
            assert_eq!(
                relative_path, "",
                "demo-root should have empty relative_path"
            );
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
    let serde_dependent_names: Vec<&str> =
        serde_dependents.iter().map(|n| n.name.as_str()).collect();
    assert!(
        serde_dependent_names.contains(&"demo-lib"),
        "serde dependents should include demo-lib, got {:?}",
        serde_dependent_names
    );

    // demo-lib's dependencies should include demo-util and serde
    let lib_deps = resolve.dependencies(&lib_node.package_id);
    let lib_dep_names: Vec<&str> = lib_deps.iter().map(|n| n.name.as_str()).collect();
    assert!(
        lib_dep_names.contains(&"demo-util"),
        "demo-lib should depend on demo-util, got {:?}",
        lib_dep_names
    );
    assert!(
        lib_dep_names.contains(&"serde"),
        "demo-lib should depend on serde, got {:?}",
        lib_dep_names
    );

    // Total node count should include all transitive deps
    let total_nodes = resolve.nodes().count();
    assert!(
        total_nodes >= 5,
        "expected at least 5 nodes (3 first-party + serde + serde_derive), got {}",
        total_nodes
    );

    // Cache construction should work
    let cache = BuckalCache::from_resolve(
        &resolve,
        &cargo_metadata::camino::Utf8PathBuf::from("/tmp/buckal-test/first-party-demo"),
    );
    // Verify cache has entries for all nodes
    let cache_str = toml::to_string_pretty(&cache).unwrap();
    assert!(
        cache_str.contains("fingerprints"),
        "cache should contain fingerprints section"
    );
}

#[test]
#[ignore]
fn test_dag_monorepo_demo() {
    let resolve = resolve_from_manifest("/tmp/buckal-test/monorepo-demo/project");

    // Should contain the 2 workspace members (virtual workspace - no root package)
    assert!(
        resolve.find_by_name("sub-lib", None).is_some(),
        "sub-lib not found"
    );
    assert!(
        resolve.find_by_name("sub-app", None).is_some(),
        "sub-app not found"
    );

    // Both should be FirstParty
    let sub_lib = resolve.find_by_name("sub-lib", None).unwrap();
    let sub_app = resolve.find_by_name("sub-app", None).unwrap();
    assert!(
        matches!(&sub_lib.kind, NodeKind::FirstParty { .. }),
        "sub-lib should be FirstParty"
    );
    assert!(
        matches!(&sub_app.kind, NodeKind::FirstParty { .. }),
        "sub-app should be FirstParty"
    );

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
    let cache = BuckalCache::from_resolve(
        &resolve,
        &cargo_metadata::camino::Utf8PathBuf::from("/tmp/buckal-test/monorepo-demo/project"),
    );
    let cache_str = toml::to_string_pretty(&cache).unwrap();
    assert!(
        cache_str.contains("fingerprints"),
        "cache should contain fingerprints section"
    );
}

#[test]
#[ignore]
fn test_dag_fd_find() {
    let resolve = resolve_from_manifest("/tmp/buckal-test/fd");

    // fd-find is the only first-party package
    let fd = resolve.find_by_name("fd-find", None).unwrap();
    assert!(
        matches!(&fd.kind, NodeKind::FirstParty { .. }),
        "fd-find should be FirstParty"
    );
    match &fd.kind {
        NodeKind::FirstParty { relative_path } => {
            assert_eq!(relative_path, "", "fd-find is at workspace root");
        }
        _ => panic!("expected FirstParty"),
    }

    // All other packages should be ThirdParty
    let third_party_count = resolve
        .nodes()
        .filter(|n| matches!(&n.kind, NodeKind::ThirdParty))
        .count();
    let first_party_count = resolve
        .nodes()
        .filter(|n| matches!(&n.kind, NodeKind::FirstParty { .. }))
        .count();
    assert_eq!(first_party_count, 1, "only fd-find is first-party");
    assert!(
        third_party_count >= 80,
        "fd has a large transitive dep graph, got {}",
        third_party_count
    );

    // Total nodes should match the full resolve graph (~103 packages)
    let total = resolve.nodes().count();
    assert!(total >= 100, "expected at least 100 nodes, got {}", total);
    assert_eq!(total, first_party_count + third_party_count);

    // Spot-check key dependencies of fd-find
    let fd_deps = resolve.dependencies(&fd.package_id);
    let fd_dep_names: Vec<&str> = fd_deps.iter().map(|n| n.name.as_str()).collect();
    for expected in &["regex", "clap", "ignore", "anyhow", "jiff"] {
        assert!(
            fd_dep_names.contains(expected),
            "fd-find should depend on {}, got {:?}",
            expected,
            fd_dep_names
        );
    }

    // clap's dependents should include fd-find
    let clap = resolve.find_by_name("clap", None).unwrap();
    assert!(matches!(&clap.kind, NodeKind::ThirdParty));
    let clap_dependents = resolve.dependents(&clap.package_id);
    let clap_dependent_names: Vec<&str> = clap_dependents.iter().map(|n| n.name.as_str()).collect();
    assert!(
        clap_dependent_names.contains(&"fd-find"),
        "clap dependents should include fd-find, got {:?}",
        clap_dependent_names
    );

    // Verify a transitive dependency chain: fd-find -> regex -> regex-syntax
    let regex_node = resolve.find_by_name("regex", None).unwrap();
    let regex_deps = resolve.dependencies(&regex_node.package_id);
    let regex_dep_names: Vec<&str> = regex_deps.iter().map(|n| n.name.as_str()).collect();
    assert!(
        regex_dep_names.contains(&"regex-syntax"),
        "regex should depend on regex-syntax, got {:?}",
        regex_dep_names
    );

    // regex-syntax's dependents should include both regex and fd-find (fd depends on it directly)
    let regex_syntax = resolve.find_by_name("regex-syntax", None).unwrap();
    let rs_dependents = resolve.dependents(&regex_syntax.package_id);
    let rs_dependent_names: Vec<&str> = rs_dependents.iter().map(|n| n.name.as_str()).collect();
    assert!(
        rs_dependent_names.contains(&"regex"),
        "regex-syntax dependents should include regex, got {:?}",
        rs_dependent_names
    );

    // find_by_name with version filtering
    let clap_version = &clap.version;
    assert!(resolve.find_by_name("clap", Some(clap_version)).is_some());
    assert!(resolve.find_by_name("clap", Some("0.0.0")).is_none());

    // Cache construction and fingerprint determinism
    let ws_root = cargo_metadata::camino::Utf8PathBuf::from("/tmp/buckal-test/fd");
    let cache1 = BuckalCache::from_resolve(&resolve, &ws_root);
    let cache2 = BuckalCache::from_resolve(&resolve, &ws_root);
    let s1 = toml::to_string_pretty(&cache1).unwrap();
    let s2 = toml::to_string_pretty(&cache2).unwrap();
    assert_eq!(
        s1, s2,
        "cache should be deterministic across repeated construction"
    );
    assert!(s1.contains("fingerprints"));

    // Diff of identical caches should produce no changes
    let diff = cache1.diff(&cache2, &ws_root);
    assert!(
        diff.changes.is_empty(),
        "diff of identical caches should be empty, got {} changes",
        diff.changes.len()
    );
}
