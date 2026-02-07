use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use clap::Parser;

use crate::{
    buck::parse_buck_file,
    buck::patch_buck_rules,
    buckal_log, buckal_note,
    buckify::{buckify_dep_node, buckify_root_node, cross, gen_buck_content, vendor_package},
    buckify::flush_root,
    cache::BuckalCache,
    config::RepoConfig,
    context::BuckalContext,
    resolve::BuckalResolve,
    utils::{UnwrapOrExit, ensure_prerequisites},
};

#[derive(Parser, Debug)]
pub struct PatchArgs {
    /// Dependency spec in the form <DEP>@<VERSION>
    pub spec: String,
}

fn parse_patch_spec(spec: &str) -> (&str, Option<&str>) {
    if let Some((name, ver)) = spec.split_once('@') {
        (name, Some(ver))
    } else {
        (spec, None)
    }
}

pub fn execute(args: &PatchArgs) {
    ensure_prerequisites().unwrap_or_exit();

    match do_execute(args) {
        Ok(()) => {}
        Err(e) => {
            crate::buckal_error!("{:#}", e);
            std::process::exit(1);
        }
    }
}

fn do_execute(args: &PatchArgs) -> Result<()> {
    let (dep_name, target_version) = parse_patch_spec(&args.spec);
    let target_version = target_version
        .ok_or_else(|| anyhow!("version is required: use <DEP>@<VERSION> format"))?;

    // Build initial context and resolve
    let ctx = BuckalContext::new();
    let resolve = BuckalResolve::from_context(&ctx);

    // Find the current node for this dependency
    let current_node = resolve
        .find_by_name(dep_name, None)
        .ok_or_else(|| anyhow!("dependency '{}' not found in the resolve graph", dep_name))?;

    let old_version = current_node.version.clone();

    // Return early if unchanged
    if old_version == target_version {
        buckal_note!("{} is already at version {}", dep_name, target_version);
        return Ok(());
    }

    buckal_log!(
        "Patching",
        format!("{} v{} -> v{}", dep_name, old_version, target_version)
    );

    // Run cargo update to change the version in Cargo.lock
    let mut cargo_cmd = Command::new("cargo");
    cargo_cmd
        .arg("update")
        .arg("-p")
        .arg(format!("{}@{}", dep_name, old_version))
        .arg("--precise")
        .arg(target_version)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cargo_cmd
        .status()
        .context("failed to execute `cargo update`")?;

    if !status.success() {
        return Err(anyhow!("cargo update exited with failure status"));
    }

    // Rebuild context and resolve with fresh metadata
    let ctx = BuckalContext::new();
    let resolve = BuckalResolve::from_context(&ctx);

    // Regenerate root BUCK
    flush_root(&ctx);

    // Regenerate BUCK file for the patched dependency
    if let Some(new_node) = resolve.find_by_name(dep_name, Some(target_version)) {
        regenerate_node_buck(&new_node.package_id, &ctx)?;
    }

    // Regenerate BUCK files for all dependents of the old package
    // We look up the patched node (at new version) and get its dependents
    if let Some(new_node) = resolve.find_by_name(dep_name, Some(target_version)) {
        let dependents = resolve.dependents(&new_node.package_id);
        for dep_node in &dependents {
            regenerate_node_buck(&dep_node.package_id, &ctx)?;
        }
    }

    // Save patch entry to buckal.toml
    RepoConfig::save_patch_entry(dep_name, &old_version, target_version)
        .context("failed to save patch entry to buckal.toml")?;

    // Build and save new cache
    let new_cache = BuckalCache::from_resolve(&resolve, &ctx.workspace_root);
    new_cache.save();

    buckal_log!(
        "Patched",
        format!("{} v{} -> v{}", dep_name, old_version, target_version)
    );

    Ok(())
}

fn regenerate_node_buck(
    pkg_id: &cargo_metadata::PackageId,
    ctx: &BuckalContext,
) -> Result<()> {
    let node = ctx
        .nodes_map
        .get(pkg_id)
        .ok_or_else(|| anyhow!("node not found for package {:?}", pkg_id))?;
    let package = ctx
        .packages_map
        .get(pkg_id)
        .ok_or_else(|| anyhow!("package not found for {:?}", pkg_id))?;

    // Skip root package
    if let Some(root) = &ctx.root {
        if pkg_id == &root.id {
            return Ok(());
        }
    }

    buckal_log!(
        "Flushing",
        format!("{} v{}", package.name, package.version)
    );

    // Determine vendor directory
    let vendor_dir = if package.source.is_none() {
        package.manifest_path.parent().unwrap().to_owned()
    } else {
        vendor_package(package)
    };

    // Generate BUCK rules
    let mut buck_rules = if package.source.is_none() {
        buckify_root_node(node, ctx)
    } else {
        buckify_dep_node(node, ctx)
    };

    // Patch BUCK Rules
    let buck_path = vendor_dir.join("BUCK");
    if buck_path.exists() {
        if !ctx.no_merge && !ctx.repo_config.patch_fields.is_empty() {
            let existing_rules =
                parse_buck_file(&buck_path).expect("Failed to parse existing BUCK file");
            patch_buck_rules(
                &existing_rules,
                &mut buck_rules,
                &ctx.repo_config.patch_fields,
            );
        }
    } else {
        std::fs::File::create(&buck_path).expect("Failed to create BUCK file");
    }

    // Generate the BUCK file
    let mut buck_content = gen_buck_content(&buck_rules);
    buck_content = cross::patch_rust_test_target_compatible_with(buck_content);
    std::fs::write(&buck_path, buck_content).expect("Failed to write BUCK file");

    Ok(())
}
