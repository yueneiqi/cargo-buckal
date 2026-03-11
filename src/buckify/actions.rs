use regex::Regex;

use crate::{
    buck::{parse_buck_file, patch_buck_rules},
    buckal_log,
    cache::{BuckalChange, ChangeType},
    context::BuckalContext,
    resolve::{BuckalNode, NodeKind},
    utils::{UnwrapOrExit, get_vendor_dir},
};

use super::{
    buckify_dep_node, buckify_root_node, cross, gen_buck_content, vendor_package, windows,
};

impl BuckalChange {
    pub fn apply(&self, ctx: &BuckalContext) {
        // This function applies changes to the BUCK files of detected packages in the cache diff, but skips the root package.
        let re: Regex = Regex::new(r"^([^+#]+)\+([^#]+)#([^@]+)@([^+#]+)(?:\+(.+))?$")
            .expect("error creating regex");
        let skip_pattern = format!("path+file://{}", ctx.workspace_root);

        for (id, change_type) in &self.changes {
            match change_type {
                ChangeType::Added | ChangeType::Changed => {
                    // Skip root package
                    if let Some(root_id) = &ctx.root
                        && id == root_id
                    {
                        continue;
                    }

                    if let Some(node) = ctx.resolve.nodes().find(|n| &n.package_id == id) {
                        buckal_log!(
                            if let ChangeType::Added = change_type {
                                "Adding"
                            } else {
                                "Flushing"
                            },
                            format!("{} v{}", node.name, node.version)
                        );

                        // Vendor package sources
                        let vendor_dir = if !is_third_party(node) {
                            node.manifest_path.parent().unwrap().to_owned()
                        } else {
                            vendor_package(node)
                        };

                        // Generate BUCK rules
                        let mut buck_rules = if !is_third_party(node) {
                            buckify_root_node(node, ctx)
                        } else {
                            buckify_dep_node(node, ctx)
                        };

                        // Patch BUCK Rules
                        let buck_path = vendor_dir.join("BUCK");
                        if buck_path.exists() {
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
                            std::fs::File::create(&buck_path).expect("Failed to create BUCK file");
                        }

                        // Generate the BUCK file
                        let mut buck_content = gen_buck_content(&buck_rules);
                        buck_content = cross::patch_rust_test_target_compatible_with(buck_content);
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
                    let vendor_dir =
                        get_vendor_dir(id).unwrap_or_exit_ctx("failed to get vendor directory");
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
    // Generate BUCK file for root package
    // Skip if root package is not found (in virtual workspace)
    if let Some(root_id) = &ctx.root {
        let root_node = ctx
            .resolve
            .nodes()
            .find(|n| &n.package_id == root_id)
            .expect("Root node not found");

        buckal_log!(
            "Flushing",
            format!("{} v{}", root_node.name, root_node.version)
        );

        let manifest_dir = root_node
            .manifest_path
            .parent()
            .expect("Failed to get manifest directory")
            .to_owned();
        let buck_path = manifest_dir.join("BUCK");

        // Generate BUCK rules
        let buck_rules = buckify_root_node(root_node, ctx);

        // Generate the BUCK file
        let mut buck_content = gen_buck_content(&buck_rules);
        buck_content = windows::patch_root_windows_rustc_flags(buck_content, ctx, root_node);
        buck_content = cross::patch_rust_test_target_compatible_with(buck_content);
        std::fs::write(&buck_path, buck_content).expect("Failed to write BUCK file");
    }
}

/// Check if a node represents a third-party dependency
pub(super) fn is_third_party(node: &BuckalNode) -> bool {
    matches!(node.kind, NodeKind::ThirdParty)
}
