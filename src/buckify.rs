use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet as Set, HashMap},
    io::{BufWriter, Write},
    sync::{Mutex, OnceLock},
};

use anyhow::{bail, Context, Result};
use cargo_metadata::{
    DependencyKind, Node, NodeDep, Package, PackageId, Target, camino::Utf8PathBuf,
};
use itertools::Itertools;
use regex::Regex;
use serde_json::Value;

use crate::{
    RUST_CRATES_ROOT,
    buck::{
        Alias, BuildscriptRun, CargoManifest, CargoTargetKind, FileGroup, Glob, HttpArchive, Load,
        Rule, RustBinary, RustLibrary, RustRule, RustTest, parse_buck_file, patch_buck_rules,
    },
    buck2::Buck2Command,
    buckal_log, buckal_warn,
    cache::{BuckalChange, ChangeType},
    context::BuckalContext,
    platform::{Os, buck_labels, lookup_platforms, oses_from_platform},
    utils::{UnwrapOrExit, get_buck2_root, get_cfgs, get_target, get_vendor_dir},
};

pub fn buckify_dep_node(node: &Node, ctx: &BuckalContext) -> Vec<Rule> {
    let package = ctx.packages_map.get(&node.id).unwrap().to_owned();

    // emit buck rules for lib target
    let mut buck_rules: Vec<Rule> = Vec::new();

    let manifest_dir = package.manifest_path.parent().unwrap().to_owned();
    let lib_target = package
        .targets
        .iter()
        .find(|t| {
            t.kind.contains(&cargo_metadata::TargetKind::Lib)
                || t.kind.contains(&cargo_metadata::TargetKind::CDyLib)
                || t.kind.contains(&cargo_metadata::TargetKind::DyLib)
                || t.kind.contains(&cargo_metadata::TargetKind::RLib)
                || t.kind.contains(&cargo_metadata::TargetKind::StaticLib)
                || t.kind.contains(&cargo_metadata::TargetKind::ProcMacro)
        })
        .expect("No library target found");

    let http_archive = emit_http_archive(&package, ctx);
    buck_rules.push(Rule::HttpArchive(http_archive));

    let cargo_manifest = emit_cargo_manifest(&package);
    buck_rules.push(Rule::CargoManifest(cargo_manifest));

    let rust_library = emit_rust_library(
        &package,
        node,
        &ctx.packages_map,
        lib_target,
        &manifest_dir,
        &package.name,
        ctx,
    );

    buck_rules.push(Rule::RustLibrary(rust_library));

    // Check if the package has a build script
    let custom_build_target = package
        .targets
        .iter()
        .find(|t| t.kind.contains(&cargo_metadata::TargetKind::CustomBuild));

    if let Some(build_target) = custom_build_target {
        // Patch the rust_library rule to support build scripts
        for rule in &mut buck_rules {
            if let Some(rust_rule) = rule.as_rust_rule_mut() {
                patch_with_buildscript(rust_rule, build_target, &package);
            }
        }

        // create the build script rule
        let buildscript_build = emit_buildscript_build(
            build_target,
            &package,
            node,
            &ctx.packages_map,
            &manifest_dir,
            ctx,
        );
        buck_rules.push(Rule::RustBinary(buildscript_build));

        // create the build script run rule
        let buildscript_run = emit_buildscript_run(&package, node, &ctx.packages_map, build_target);
        buck_rules.push(Rule::BuildscriptRun(buildscript_run));
    }

    buck_rules
}

pub fn buckify_root_node(node: &Node, ctx: &BuckalContext) -> Vec<Rule> {
    let package = ctx.packages_map.get(&node.id).unwrap().to_owned();

    let bin_targets = package
        .targets
        .iter()
        .filter(|t| t.kind.contains(&cargo_metadata::TargetKind::Bin))
        .collect::<Vec<_>>();

    let lib_targets = package
        .targets
        .iter()
        .filter(|t| {
            t.kind.contains(&cargo_metadata::TargetKind::Lib)
                || t.kind.contains(&cargo_metadata::TargetKind::CDyLib)
                || t.kind.contains(&cargo_metadata::TargetKind::DyLib)
                || t.kind.contains(&cargo_metadata::TargetKind::RLib)
                || t.kind.contains(&cargo_metadata::TargetKind::StaticLib)
                || t.kind.contains(&cargo_metadata::TargetKind::ProcMacro)
        })
        .collect::<Vec<_>>();

    let test_targets = package
        .targets
        .iter()
        .filter(|t| t.kind.contains(&cargo_metadata::TargetKind::Test))
        .collect::<Vec<_>>();

    let mut buck_rules: Vec<Rule> = Vec::new();

    let manifest_dir = package.manifest_path.parent().unwrap().to_owned();

    // emit filegroup rule for vendor
    let filegroup = emit_filegroup(&package);
    buck_rules.push(Rule::FileGroup(filegroup));

    let cargo_manifest = emit_cargo_manifest(&package);
    buck_rules.push(Rule::CargoManifest(cargo_manifest));

    // emit buck rules for bin targets
    for bin_target in &bin_targets {
        let buckal_name = bin_target.name.to_owned();

        let mut rust_binary = emit_rust_binary(
            &package,
            node,
            &ctx.packages_map,
            bin_target,
            &manifest_dir,
            &buckal_name,
            ctx,
        );

        if lib_targets.iter().any(|l| l.name == bin_target.name) {
            // Cargo allows `main.rs` to use items from `lib.rs` via the crate's own name by default.
            rust_binary
                .deps_mut()
                .insert(format!(":lib{}", bin_target.name));
        }

        buck_rules.push(Rule::RustBinary(rust_binary));
    }

    // emit buck rules for lib targets
    for lib_target in &lib_targets {
        let buckal_name = if bin_targets.iter().any(|b| b.name == lib_target.name) {
            format!("lib{}", lib_target.name)
        } else {
            lib_target.name.to_owned()
        };

        let rust_library = emit_rust_library(
            &package,
            node,
            &ctx.packages_map,
            lib_target,
            &manifest_dir,
            &buckal_name,
            ctx,
        );

        buck_rules.push(Rule::RustLibrary(rust_library));

        if !ctx.repo_config.ignore_tests && lib_target.test {
            // If the library target has inline tests, emit a rust_test rule for it
            let buckal_name = format!("{}-unittest", lib_target.name);

            let rust_test = emit_rust_test(
                &package,
                node,
                &ctx.packages_map,
                lib_target,
                &manifest_dir,
                &buckal_name,
                ctx,
            );

            buck_rules.push(Rule::RustTest(rust_test));
        }
    }

    // emit buck rules for integration test
    if !ctx.repo_config.ignore_tests {
        for test_target in &test_targets {
            let buckal_name = test_target.name.to_owned();

            let mut rust_test = emit_rust_test(
                &package,
                node,
                &ctx.packages_map,
                test_target,
                &manifest_dir,
                &buckal_name,
                ctx,
            );

            let package_name = package.name.replace("-", "_");
            let mut lib_alias = false;
            if bin_targets.iter().any(|b| b.name == package_name) {
                lib_alias = true;
                rust_test.env_mut().insert(
                    format!("CARGO_BIN_EXE_{}", package_name),
                    format!("$(location :{})", package_name),
                );
            }
            if lib_targets.iter().any(|l| l.name == package_name) {
                if lib_alias {
                    rust_test.deps_mut().insert(format!(":lib{}", package_name));
                } else {
                    rust_test.deps_mut().insert(format!(":{}", package_name));
                }
            }

            buck_rules.push(Rule::RustTest(rust_test));
        }
    }

    // Check if the package has a build script
    let custom_build_target = package
        .targets
        .iter()
        .find(|t| t.kind.contains(&cargo_metadata::TargetKind::CustomBuild));

    if let Some(build_target) = custom_build_target {
        // Patch the rust_library and rust_binary rules to support build scripts
        for rule in &mut buck_rules {
            if let Some(rust_rule) = rule.as_rust_rule_mut() {
                patch_with_buildscript(rust_rule, build_target, &package);
            }
        }

        // create the build script rule
        let buildscript_build = emit_buildscript_build(
            build_target,
            &package,
            node,
            &ctx.packages_map,
            &manifest_dir,
            ctx,
        );
        buck_rules.push(Rule::RustBinary(buildscript_build));

        // create the build script run rule
        let buildscript_run = emit_buildscript_run(&package, node, &ctx.packages_map, build_target);
        buck_rules.push(Rule::BuildscriptRun(buildscript_run));
    }

    buck_rules
}

pub fn vendor_package(package: &Package) -> Utf8PathBuf {
    // Vendor the package sources to `third-party/rust/crates/<package_name>/<version>`
    let vendor_dir = get_vendor_dir(&package.name, &package.version.to_string())
        .unwrap_or_exit_ctx("failed to get vendor directory");
    if !vendor_dir.exists() {
        std::fs::create_dir_all(&vendor_dir).expect("Failed to create target directory");
    }

    vendor_dir
}

pub fn gen_buck_content(rules: &[Rule]) -> String {
    let loads: Vec<Rule> = vec![
        Rule::Load(Load {
            bzl: "@buckal//:cargo_manifest.bzl".to_owned(),
            items: Set::from(["cargo_manifest".to_owned()]),
        }),
        Rule::Load(Load {
            bzl: "@buckal//:wrapper.bzl".to_owned(),
            items: Set::from([
                "buildscript_run".to_owned(),
                "rust_binary".to_owned(),
                "rust_library".to_owned(),
                "rust_test".to_owned(),
            ]),
        }),
    ];

    let loads_string = loads
        .iter()
        .map(serde_starlark::to_string)
        .map(Result::unwrap)
        .join("");

    let mut content = rules
        .iter()
        .map(serde_starlark::to_string)
        .map(Result::unwrap)
        .join("\n");

    content.insert(0, '\n');
    content.insert_str(0, &loads_string);
    content.insert_str(0, "# @generated by `cargo buckal`\n\n");
    content
}

fn dep_kind_matches(target_kind: CargoTargetKind, dep_kind: DependencyKind) -> bool {
    match target_kind {
        CargoTargetKind::CustomBuild => dep_kind == DependencyKind::Build,
        CargoTargetKind::Test => dep_kind == DependencyKind::Development,
        _ => dep_kind == DependencyKind::Normal,
    }
}

static FIRST_PARTY_LABEL_CACHE: OnceLock<Mutex<HashMap<Utf8PathBuf, String>>> = OnceLock::new();

fn first_party_label_cache() -> &'static Mutex<HashMap<Utf8PathBuf, String>> {
    FIRST_PARTY_LABEL_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn resolve_first_party_label(dep_package: &Package) -> Result<String> {
    let manifest_path = dep_package.manifest_path.clone();
    if let Some(cached) = first_party_label_cache()
        .lock()
        .expect("first_party_label_cache poisoned")
        .get(&manifest_path)
        .cloned()
    {
        return Ok(cached);
    }

    let buck2_root = get_buck2_root().context("failed to get buck2 root")?;
    let manifest_dir = dep_package
        .manifest_path
        .parent()
        .context("manifest_path should always have a parent directory")?;
    let relative = manifest_dir.strip_prefix(&buck2_root).with_context(|| {
        format!(
            "dependency manifest dir `{}` is not under Buck2 root `{}`",
            manifest_dir, buck2_root
        )
    })?;

    let relative_path = relative.as_str();
    let target = if relative_path.is_empty() {
        "//...".to_string()
    } else {
        format!("//{relative_path}/...")
    };

    let output = Buck2Command::targets()
        .arg(&target)
        .arg("--json")
        .output()
        .with_context(|| format!("failed to execute `buck2 targets {target} --json`"))?;
    if !output.status.success() {
        bail!(
            "buck2 targets failed for `{target}`:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let targets: Vec<Value> = serde_json::from_slice(&output.stdout)
        .context("failed to parse buck2 targets JSON output")?;
    let target_item = targets
        .iter()
        .find(|t| {
            t.get("buck.type")
                .and_then(|k| k.as_str())
                .is_some_and(|k| k.ends_with("rust_library"))
        })
        .with_context(|| {
            format!(
                "failed to find `rust_library` target for package `{}` (manifest `{}`) using target pattern `{target}`",
                dep_package.name, dep_package.manifest_path
            )
        })?;

    let buck_package_raw = target_item
        .get("buck.package")
        .and_then(|n| n.as_str())
        .context("buck2 targets output is missing `buck.package`")?;
    let buck_package = buck_package_raw
        .strip_prefix("root")
        .unwrap_or(buck_package_raw);

    let buck_name = target_item
        .get("name")
        .and_then(|n| n.as_str())
        .context("buck2 targets output is missing `name`")?;

    let label = format!("{buck_package}:{buck_name}");

    first_party_label_cache()
        .lock()
        .expect("first_party_label_cache poisoned")
        .insert(manifest_path, label.clone());

    Ok(label)
}

fn resolve_dep_label(
    dep: &NodeDep,
    dep_package: &Package,
    use_workspace_alias: bool,
) -> Result<(String, Option<String>)> {
    let dep_package_name = dep_package.name.to_string();
    let is_renamed = dep.name != dep_package_name.replace("-", "_");
    let alias = if is_renamed {
        Some(dep.name.clone())
    } else {
        None
    };

    if dep_package.source.is_none() {
        let label = resolve_first_party_label(dep_package).with_context(|| {
            format!(
                "failed to resolve first-party label for `{}`",
                dep_package.name
            )
        })?;
        Ok((label, alias))
    } else {
        // third-party dependency
        Ok((
            if use_workspace_alias {
                format!("//third-party/rust:{}", dep_package.name)
            } else {
                format!(
                    "//{RUST_CRATES_ROOT}/{}/{}:{}",
                    dep_package.name, dep_package.version, dep_package.name
                )
            },
            alias,
        ))
    }
}

fn insert_dep(
    rust_rule: &mut dyn RustRule,
    target: &str,
    alias: Option<&str>,
    platforms: Option<&std::collections::BTreeSet<Os>>,
) {
    if let Some(platforms) = platforms {
        for os in platforms {
            let os_key = os.key().to_owned();
            if let Some(alias) = alias {
                let entries = rust_rule
                    .os_named_deps_mut()
                    .entry(alias.to_owned())
                    .or_default();

                if let Some(existing) = entries.get(&os_key) {
                    if existing != target {
                        buckal_warn!(
                            "os_named_deps alias '{}' had conflicting targets for platform '{}': '{}' vs '{}'",
                            alias,
                            os_key,
                            existing,
                            target
                        );
                    }
                } else {
                    entries.insert(os_key.clone(), target.to_owned());
                }
            } else {
                rust_rule
                    .os_deps_mut()
                    .entry(os_key)
                    .or_default()
                    .insert(target.to_owned());
            }
        }
    } else if let Some(alias) = alias {
        let entry = rust_rule.named_deps_mut().entry(alias.to_owned());
        match entry {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(target.to_owned());
            }
            std::collections::btree_map::Entry::Occupied(o) => {
                if o.get() != target {
                    buckal_warn!(
                        "named_deps alias '{}' had conflicting targets: '{}' vs '{}'",
                        alias,
                        o.get(),
                        target
                    );
                }
            }
        }
    } else {
        rust_rule.deps_mut().insert(target.to_owned());
    }
}

fn set_deps(
    rust_rule: &mut dyn RustRule,
    node: &Node,
    packages_map: &HashMap<PackageId, Package>,
    kind: CargoTargetKind,
    ctx: &BuckalContext,
) {
    let use_workspace_alias = ctx.repo_config.inherit_workspace_deps && node.id == ctx.root.id;

    for dep in &node.deps {
        let Some(dep_package) = packages_map.get(&dep.pkg) else {
            continue;
        };

        let mut unconditional = false;
        let mut platforms = Set::<Os>::new();
        let mut dropped_due_to_unsupported = false;

        for dk in dep
            .dep_kinds
            .iter()
            .filter(|dk| dep_kind_matches(kind, dk.kind))
        {
            match &dk.target {
                None => unconditional = true,
                Some(platform) => {
                    let oses = oses_from_platform(platform);
                    if oses.is_empty() {
                        // Only drop unsupported platforms if the flag is set
                        if ctx.supported_platform_only {
                            dropped_due_to_unsupported = true;
                            continue;
                        }
                        // If flag is not set, include the empty platform set
                        platforms.extend(oses);
                    } else {
                        platforms.extend(oses);
                    }
                }
            }
        }

        if !unconditional && platforms.is_empty() {
            if dropped_due_to_unsupported {
                buckal_warn!(
                    "Dependency '{}' (package '{}') targets only unsupported platforms and will be omitted.",
                    dep.name,
                    dep_package.name
                );
            }
            continue;
        }

        let (target_label, alias) =
            resolve_dep_label(dep, dep_package, use_workspace_alias).unwrap_or_exit_ctx(format!(
                "failed to resolve dependency label for '{}' (package '{}')",
                dep.name, dep_package.name
            ));

        if unconditional {
            insert_dep(rust_rule, &target_label, alias.as_deref(), None);
        } else {
            insert_dep(rust_rule, &target_label, alias.as_deref(), Some(&platforms));
        }
    }
}

/// Emit `rust_library` rule for the given lib target
fn emit_rust_library(
    package: &Package,
    node: &Node,
    packages_map: &HashMap<PackageId, Package>,
    lib_target: &Target,
    manifest_dir: &Utf8PathBuf,
    buckal_name: &str,
    ctx: &BuckalContext,
) -> RustLibrary {
    let mut rust_library = RustLibrary {
        name: buckal_name.to_owned(),
        srcs: Set::from([get_vendor_target(package)]),
        crate_name: lib_target.name.to_owned().replace("-", "_"),
        edition: package.edition.to_string(),
        features: Set::from_iter(node.features.iter().map(|f| f.to_string())),
        rustc_flags: Set::from([format!(
            "@$(location :{}-manifest[env_flags])",
            package.name
        )]),
        visibility: Set::from(["PUBLIC".to_owned()]),
        ..Default::default()
    };

    if lib_target
        .kind
        .contains(&cargo_metadata::TargetKind::ProcMacro)
    {
        rust_library.proc_macro = Some(true);
    }

    // Set the crate root path
    rust_library.crate_root = format!(
        "vendor/{}",
        lib_target
            .src_path
            .to_owned()
            .strip_prefix(manifest_dir)
            .expect("Failed to get library source path")
            .as_str()
            .replace('\\', "/")
    );

    // look up platform compatibility
    if let Some(platforms) = lookup_platforms(&package.name) {
        rust_library.compatible_with = buck_labels(&platforms);
    }

    // Set dependencies
    set_deps(
        &mut rust_library,
        node,
        packages_map,
        CargoTargetKind::Lib,
        ctx,
    );
    rust_library
}

/// Emit `rust_binary` rule for the given bin target
fn emit_rust_binary(
    package: &Package,
    node: &Node,
    packages_map: &HashMap<PackageId, Package>,
    bin_target: &Target,
    manifest_dir: &Utf8PathBuf,
    buckal_name: &str,
    ctx: &BuckalContext,
) -> RustBinary {
    let mut rust_binary = RustBinary {
        name: buckal_name.to_owned(),
        srcs: Set::from([get_vendor_target(package)]),
        crate_name: bin_target.name.to_owned().replace("-", "_"),
        edition: package.edition.to_string(),
        features: Set::from_iter(node.features.iter().map(|f| f.to_string())),
        rustc_flags: Set::from([format!(
            "@$(location :{}-manifest[env_flags])",
            package.name
        )]),
        visibility: Set::from(["PUBLIC".to_owned()]),
        ..Default::default()
    };

    // Set the crate root path
    rust_binary.crate_root = format!(
        "vendor/{}",
        bin_target
            .src_path
            .to_owned()
            .strip_prefix(manifest_dir)
            .expect("Failed to get binary source path")
            .as_str()
            .replace('\\', "/")
    );

    // Set dependencies
    set_deps(
        &mut rust_binary,
        node,
        packages_map,
        CargoTargetKind::Bin,
        ctx,
    );

    if let Some(platforms) = lookup_platforms(&package.name) {
        rust_binary.compatible_with = buck_labels(&platforms);
    }
    rust_binary
}

/// Emit `rust_test` rule for the given bin target
fn emit_rust_test(
    package: &Package,
    node: &Node,
    packages_map: &HashMap<PackageId, Package>,
    test_target: &Target,
    manifest_dir: &Utf8PathBuf,
    buckal_name: &str,
    ctx: &BuckalContext,
) -> RustTest {
    let mut rust_test = RustTest {
        name: buckal_name.to_owned(),
        srcs: Set::from([get_vendor_target(package)]),
        crate_name: test_target.name.to_owned().replace("-", "_"),
        edition: package.edition.to_string(),
        features: Set::from_iter(node.features.iter().map(|f| f.to_string())),
        rustc_flags: Set::from([format!(
            "@$(location :{}-manifest[env_flags])",
            package.name
        )]),
        visibility: Set::from(["PUBLIC".to_owned()]),
        ..Default::default()
    };

    // Set the crate root path
    rust_test.crate_root = format!(
        "vendor/{}",
        test_target
            .src_path
            .to_owned()
            .strip_prefix(manifest_dir)
            .expect("Failed to get binary source path")
            .as_str()
            .replace('\\', "/")
    );

    // Set dependencies
    set_deps(
        &mut rust_test,
        node,
        packages_map,
        CargoTargetKind::Test,
        ctx,
    );

    if let Some(platforms) = lookup_platforms(&package.name) {
        rust_test.compatible_with = buck_labels(&platforms);
    }
    rust_test
}

/// Emit `buildscript_build` rule for the given build target
fn emit_buildscript_build(
    build_target: &Target,
    package: &Package,
    node: &Node,
    packages_map: &HashMap<PackageId, Package>,
    manifest_dir: &Utf8PathBuf,
    ctx: &BuckalContext,
) -> RustBinary {
    // create the build script rule
    let mut buildscript_build = RustBinary {
        name: format!("{}-{}", package.name, build_target.name),
        srcs: Set::from([get_vendor_target(package)]),
        crate_name: build_target.name.to_owned().replace("-", "_"),
        edition: package.edition.to_string(),
        features: Set::from_iter(node.features.iter().map(|f| f.to_string())),
        rustc_flags: Set::from([format!(
            "@$(location :{}-manifest[env_flags])",
            package.name
        )]),
        ..Default::default()
    };

    // Set the crate root path for the build script
    buildscript_build.crate_root = format!(
        "vendor/{}",
        build_target
            .src_path
            .to_owned()
            .strip_prefix(manifest_dir)
            .expect("Failed to get library source path")
            .as_str()
            .replace('\\', "/")
    );

    // Set dependencies for the build script
    set_deps(
        &mut buildscript_build,
        node,
        packages_map,
        CargoTargetKind::CustomBuild,
        ctx,
    );

    buildscript_build
}

/// Emit `buildscript_run` rule for the given build target
fn emit_buildscript_run(
    package: &Package,
    node: &Node,
    packages_map: &HashMap<PackageId, Package>,
    build_target: &Target,
) -> BuildscriptRun {
    // create the build script run rule
    let build_name = get_build_name(&build_target.name);
    let mut buildscript_run = BuildscriptRun {
        name: format!("{}-{}-run", package.name, build_name),
        package_name: package.name.to_string(),
        buildscript_rule: format!(":{}-{}", package.name, build_target.name),
        env_srcs: Set::from([format!(":{}-manifest[env_dict]", package.name)]),
        features: Set::from_iter(node.features.iter().map(|f| f.to_string())),
        version: package.version.to_string(),
        manifest_dir: format!(":{}-vendor", package.name),
        visibility: Set::from(["PUBLIC".to_owned()]),
        ..Default::default()
    };

    let host_target = get_target();
    let host_cfgs = get_cfgs();

    // Set environment variables from dependencies
    // See https://doc.rust-lang.org/cargo/reference/build-scripts.html#the-links-manifest-key
    for dep in &node.deps {
        if let Some(dep_package) = packages_map.get(&dep.pkg)
            && dep_package.links.is_some()
            && dep.dep_kinds.iter().any(|dk| {
                dep_kind_matches(CargoTargetKind::Lib, dk.kind)
                    && dk
                        .target
                        .as_ref()
                        .map(|platform| platform.matches(&host_target, &host_cfgs[..]))
                        .unwrap_or(true)
            })
        {
            // Only normal dependencies with The links Manifest Key for current arch are considered
            let custom_build_target_dep = dep_package
                .targets
                .iter()
                .find(|t| t.kind.contains(&cargo_metadata::TargetKind::CustomBuild));
            if let Some(build_target_dep) = custom_build_target_dep {
                let build_name_dep = get_build_name(&build_target_dep.name);
                buildscript_run.env_srcs.insert(format!(
                    "//{RUST_CRATES_ROOT}/{}/{}:{}-{build_name_dep}-run[metadata]",
                    dep_package.name, dep_package.version, dep_package.name
                ));
            } else {
                panic!(
                    "Dependency {} has links key but no build script target",
                    dep_package.name
                );
            }
        }
    }

    buildscript_run
}

/// Patch the given `rust_library` or `rust_binary` rule to support build scripts
fn patch_with_buildscript(rust_rule: &mut dyn RustRule, build_target: &Target, package: &Package) {
    let build_name = get_build_name(&build_target.name);
    rust_rule.env_mut().insert(
        "OUT_DIR".to_owned(),
        format!("$(location :{}-{build_name}-run[out_dir])", package.name).to_owned(),
    );
    rust_rule.rustc_flags_mut().insert(
        format!(
            "@$(location :{}-{build_name}-run[rustc_flags])",
            package.name
        )
        .to_owned(),
    );
}

/// Emit `http_archive` rule for the given package
fn emit_http_archive(package: &Package, ctx: &BuckalContext) -> HttpArchive {
    let vendor_name = format!("{}-vendor", package.name);
    let url = format!(
        "https://static.crates.io/crates/{}/{}-{}.crate",
        package.name, package.name, package.version
    );
    let buckal_name = format!("{}-{}", package.name, package.version);
    let checksum = ctx
        .checksums_map
        .get(&format!("{}-{}", package.name, package.version))
        .unwrap();

    HttpArchive {
        name: vendor_name,
        urls: Set::from([url]),
        sha256: checksum.to_string(),
        _type: "tar.gz".to_owned(),
        strip_prefix: buckal_name,
        out: Some("vendor".to_owned()),
    }
}

/// Emit `filegroup` rule for the given package
fn emit_filegroup(package: &Package) -> FileGroup {
    let vendor_name = format!("{}-vendor", package.name);
    FileGroup {
        name: vendor_name,
        srcs: Glob {
            include: Set::from(["**/**".to_owned()]),
            ..Default::default()
        },
        out: Some("vendor".to_owned()),
    }
}

// Emit `cargo_manifest` rule for the given package
fn emit_cargo_manifest(package: &Package) -> CargoManifest {
    CargoManifest {
        name: format!("{}-manifest", package.name),
        vendor: get_vendor_target(package),
    }
}

fn get_build_name(s: &str) -> Cow<'_, str> {
    if let Some(stripped) = s.strip_suffix("-build") {
        Cow::Owned(stripped.to_string())
    } else {
        Cow::Borrowed(s)
    }
}

fn get_vendor_target(package: &Package) -> String {
    format!(":{}-vendor", package.name)
}

impl BuckalChange {
    pub fn apply(&self, ctx: &BuckalContext) {
        // This function applies changes to the BUCK files of detected packages in the cache diff, but skips the root package.
        let re = Regex::new(r"^([^+#]+)\+([^#]+)#([^@]+)@([^+#]+)(?:\+(.+))?$")
            .expect("error creating regex");
        let skip_pattern = format!("path+file://{}", ctx.workspace_root);

        for (id, change_type) in &self.changes {
            match change_type {
                ChangeType::Added | ChangeType::Changed => {
                    // Skip root package
                    if id == &ctx.root.id {
                        continue;
                    }

                    if let Some(node) = ctx.nodes_map.get(id) {
                        let package = ctx.packages_map.get(id).unwrap();

                        if ctx.separate && package.source.is_none() {
                            // Skip first-party packages if `--separate` is set
                            continue;
                        }

                        buckal_log!(
                            if let ChangeType::Added = change_type {
                                "Adding"
                            } else {
                                "Flushing"
                            },
                            format!("{} v{}", package.name, package.version)
                        );

                        // Vendor package sources
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
                            // buckal_warn!("test: buck_path exits: {}", buck_path);
                            // Skip merging manual changes if `--no-merge` is set
                            if !ctx.no_merge && !ctx.repo_config.patch_fields.is_empty() {
                                let existing_rules = parse_buck_file(&buck_path)
                                    .expect("Failed to parse existing BUCK file");
                                patch_buck_rules(
                                    &existing_rules,
                                    &mut buck_rules,
                                    &ctx.repo_config.patch_fields,
                                );
                            }
                        } else {
                            // buckal_warn!("test: buck_path not exit: {}", buck_path);
                            std::fs::File::create(&buck_path).expect("Failed to create BUCK file");
                        }

                        // Generate the BUCK file
                        let buck_content = gen_buck_content(&buck_rules);
                        std::fs::write(&buck_path, buck_content)
                            .expect("Failed to write BUCK file");
                    }
                }
                ChangeType::Removed => {
                    // Skip workspace_root package
                    if id.repr.starts_with(skip_pattern.as_str()) {
                        continue;
                    }

                    let caps = re.captures(&id.repr).expect("Failed to parse package ID");
                    let name = &caps[3];
                    let version = &caps[4];

                    buckal_log!("Removing", format!("{} v{}", name, version));
                    let vendor_dir = get_vendor_dir(name, version)
                        .unwrap_or_exit_ctx("failed to get vendor directory");
                    if vendor_dir.exists() {
                        std::fs::remove_dir_all(&vendor_dir)
                            .expect("Failed to remove vendor directory");
                    }
                    if let Some(package_dir) = vendor_dir.parent()
                        && package_dir.exists()
                        && package_dir.read_dir().unwrap().next().is_none()
                    {
                        std::fs::remove_dir_all(package_dir)
                            .expect("Failed to remove empty package directory");
                    }
                }
            }
        }
    }
}

pub fn flush_root(ctx: &BuckalContext) {
    buckal_log!(
        "Flushing",
        format!("{} v{}", ctx.root.name, ctx.root.version)
    );
    let root_node = ctx
        .nodes_map
        .get(&ctx.root.id)
        .expect("Root node not found");
    if ctx.repo_config.inherit_workspace_deps {
        buckal_log!(
            "Generating",
            "third-party alias rules (inherit_workspace_deps=true)"
        );
        generate_third_party_aliases(ctx);
    } else {
        buckal_log!(
            "Skipping",
            "third-party alias generation (inherit_workspace_deps=false)"
        );
    }

    let cwd = std::env::current_dir().expect("Failed to get current directory");
    let buck_path = Utf8PathBuf::from(cwd.to_str().unwrap()).join("BUCK");

    // Generate BUCK rules
    let buck_rules = buckify_root_node(root_node, ctx);

    // Generate the BUCK file
    let mut buck_content = gen_buck_content(&buck_rules);
    buck_content = patch_root_windows_rustc_flags(buck_content, ctx);
    std::fs::write(&buck_path, buck_content).expect("Failed to write BUCK file");
}

#[derive(Default)]
struct WindowsImportLibFlags {
    gnu: Vec<String>,
    msvc_x86_64: Vec<String>,
    msvc_i686: Vec<String>,
    msvc_aarch64: Vec<String>,
}

fn patch_root_windows_rustc_flags(mut buck_content: String, ctx: &BuckalContext) -> String {
    let bin_names: Vec<String> = ctx
        .root
        .targets
        .iter()
        .filter(|t| t.kind.contains(&cargo_metadata::TargetKind::Bin))
        .map(|t| t.name.clone())
        .collect();

    if bin_names.is_empty() {
        return buck_content;
    }

    let flags = windows_import_lib_flags(ctx);
    let select_expr = render_windows_rustc_flags_select(&flags);
    if select_expr.is_empty() {
        return buck_content;
    }

    for bin_name in bin_names {
        buck_content = patch_rust_binary_rustc_flags(&buck_content, &bin_name, &select_expr);
    }

    buck_content
}

fn windows_import_lib_flags(ctx: &BuckalContext) -> WindowsImportLibFlags {
    let mut flags = WindowsImportLibFlags::default();

    let add_all = |name: &str, out: &mut Vec<String>| {
        let mut matches: Vec<_> = ctx
            .packages_map
            .values()
            .filter(|p| p.name.to_string() == name)
            .collect();
        matches.sort_by(|a, b| a.version.cmp(&b.version));
        for package in matches {
            let package_name = package.name.to_string();
            out.push(format!(
                "@$(location //{}/{}/{}:{}-build-script-run[rustc_flags])",
                RUST_CRATES_ROOT, package_name, package.version, package_name
            ));
        }
    };

    // GNU targets.
    add_all("windows_x86_64_gnu", &mut flags.gnu);
    add_all(
        "winapi-x86_64-pc-windows-gnu",
        &mut flags.gnu,
    );

    // MSVC targets (per CPU).
    add_all("windows_x86_64_msvc", &mut flags.msvc_x86_64);
    add_all("windows_i686_msvc", &mut flags.msvc_i686);
    add_all("windows_aarch64_msvc", &mut flags.msvc_aarch64);

    flags
}

fn render_windows_rustc_flags_select(flags: &WindowsImportLibFlags) -> String {
    if flags.gnu.is_empty()
        && flags.msvc_x86_64.is_empty()
        && flags.msvc_i686.is_empty()
        && flags.msvc_aarch64.is_empty()
    {
        return String::new();
    }

    let mut out = String::new();
    out.push_str("select({\n");
    out.push_str("        \"prelude//os/constraints:windows\": select({\n");

    // GNU branch.
    out.push_str("            \"prelude//abi/constraints:gnu\": [\n");
    for f in &flags.gnu {
        out.push_str(&format!("                \"{}\",\n", f));
    }
    out.push_str("            ],\n");

    // MSVC branch (cpu-specific).
    out.push_str("            \"prelude//abi/constraints:msvc\": select({\n");
    out.push_str("                \"prelude//cpu/constraints:arm64\": [\n");
    for f in &flags.msvc_aarch64 {
        out.push_str(&format!("                    \"{}\",\n", f));
    }
    out.push_str("                ],\n");
    out.push_str("                \"prelude//cpu/constraints:x86_32\": [\n");
    for f in &flags.msvc_i686 {
        out.push_str(&format!("                    \"{}\",\n", f));
    }
    out.push_str("                ],\n");
    out.push_str("                \"DEFAULT\": [\n");
    for f in &flags.msvc_x86_64 {
        out.push_str(&format!("                    \"{}\",\n", f));
    }
    out.push_str("                ],\n");
    out.push_str("            }),\n");

    // Default branch (when ABI constraint is missing).
    out.push_str("            \"DEFAULT\": select({\n");
    out.push_str("                \"prelude//cpu/constraints:arm64\": [\n");
    for f in &flags.msvc_aarch64 {
        out.push_str(&format!("                    \"{}\",\n", f));
    }
    out.push_str("                ],\n");
    out.push_str("                \"prelude//cpu/constraints:x86_32\": [\n");
    for f in &flags.msvc_i686 {
        out.push_str(&format!("                    \"{}\",\n", f));
    }
    out.push_str("                ],\n");
    out.push_str("                \"DEFAULT\": [\n");
    for f in &flags.msvc_x86_64 {
        out.push_str(&format!("                    \"{}\",\n", f));
    }
    out.push_str("                ],\n");
    out.push_str("            }),\n");

    out.push_str("        }),\n");
    out.push_str("        \"DEFAULT\": [],\n");
    out.push_str("    })");

    out
}

fn patch_rust_binary_rustc_flags(buck_content: &str, bin_name: &str, select_expr: &str) -> String {
    let name_marker = format!("    name = \"{bin_name}\",");
    let Some(name_pos) = buck_content.find(&name_marker) else {
        return buck_content.to_owned();
    };

    let Some(block_start) = buck_content[..name_pos].rfind("rust_binary(\n") else {
        return buck_content.to_owned();
    };

    let Some(rustc_flags_pos) = buck_content[name_pos..].find("    rustc_flags = [") else {
        return buck_content.to_owned();
    };
    let rustc_flags_pos = name_pos + rustc_flags_pos;

    let after_rustc_flags = rustc_flags_pos + "    rustc_flags = [".len();
    let Some(list_end_rel) = buck_content[after_rustc_flags..].find("\n    ],\n") else {
        return buck_content.to_owned();
    };
    let list_end = after_rustc_flags + list_end_rel + "\n    ]".len();

    // Ensure we're patching the rust_binary block that actually belongs to this target.
    if block_start > rustc_flags_pos {
        return buck_content.to_owned();
    }

    let mut out = String::with_capacity(buck_content.len() + select_expr.len() + 64);
    out.push_str(&buck_content[..list_end]);
    out.push_str(" + ");
    out.push_str(select_expr);
    out.push_str(&buck_content[list_end..]);
    out
}

pub fn generate_third_party_aliases(ctx: &BuckalContext) {
    let root = get_buck2_root().expect("failed to get buck2 root");
    let dir = root.join("third-party/rust");
    std::fs::create_dir_all(&dir).expect("failed to create third-party/rust dir");

    let buck_file = dir.join("BUCK");

    let mut grouped: BTreeMap<String, Vec<&cargo_metadata::Package>> = BTreeMap::new();

    for (pkg_id, pkg) in &ctx.packages_map {
        // only workspace members (first-party)
        if pkg.source.is_some() {
            continue;
        }

        let node = match ctx.nodes_map.get(pkg_id) {
            Some(n) => n,
            None => continue,
        };

        for dep in &node.deps {
            let dep_pkg = ctx.packages_map.get(&dep.pkg).unwrap();
            if dep_pkg.source.is_some() {
                grouped
                    .entry(dep_pkg.name.to_string())
                    .or_default()
                    .push(dep_pkg);
            }
        }
    }

    let file = std::fs::File::create(&buck_file).expect("failed to create third-party/rust/BUCK");
    let mut writer = BufWriter::new(file);

    writeln!(writer, "# @generated by cargo-buckal\n").expect("failed to write header");

    for (crate_name, mut versions) in grouped {
        versions.sort_by(|a, b| a.version.cmp(&b.version));
        let latest = versions.last().expect("empty version list");

        let actual = format!(
            "//third-party/rust/crates/{}/{}:{}",
            crate_name, latest.version, crate_name
        );

        let rule = Alias {
            name: crate_name.clone(),
            actual,
            visibility: ["PUBLIC"].into_iter().map(String::from).collect(),
        };
        let rendered = serde_starlark::to_string(&rule).expect("failed to serialize alias");
        writeln!(writer, "{}", rendered).expect("write failed");
    }

    writer.flush().expect("failed to flush alias rules");

    buckal_log!(
        "Generated",
        format!("third-party alias rules at {}", buck_file)
    );
}
