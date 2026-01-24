use std::{fs::OpenOptions, io::Write};

use clap::Parser;

use crate::{
    RUST_CRATES_ROOT,
    assets::extract_buck2_assets,
    buck2::Buck2Command,
    buckal_error,
    buckify::flush_root,
    bundles::{fetch_buckal_cell, init_buckal_cell, init_modifier},
    cache::BuckalCache,
    context::BuckalContext,
    utils::{UnwrapOrExit, ensure_prerequisites, get_buck2_root},
};

#[derive(Parser, Debug)]
pub struct MigrateArgs {
    /// Do not use cached data from previous runs
    #[clap(long, name = "no-cache")]
    pub no_cache: bool,
    /// Merge manual edits with generated content
    #[clap(long)]
    pub merge: bool,
    /// Migrate with buck2 initialized
    #[clap(long, conflicts_with = "fetch")]
    pub buck2: bool,
    /// Fetch latest bundles from remote repository
    #[clap(long)]
    pub fetch: bool,
    /// Process first-party crates separately
    #[clap(long)]
    pub separate: bool,
}

pub fn execute(args: &MigrateArgs) {
    // Ensure all prerequisites are installed before proceeding
    ensure_prerequisites().unwrap_or_exit();

    // Initialize Buck2 project if requested
    // Compared to `cargo buckal init`, here we only setup Buck2 related files
    if args.buck2 {
        let cwd = std::env::current_dir().unwrap_or_exit();
        let existing_root = get_buck2_root().ok();
        let root_for_checks = existing_root
            .as_ref()
            .map(|root| root.as_std_path())
            .unwrap_or_else(|| cwd.as_path());
        let toolchains_dir = root_for_checks.join("toolchains");
        let platforms_dir = root_for_checks.join("platforms");
        if toolchains_dir.is_dir() || platforms_dir.is_dir() {
            buckal_error!(
                "`toolchains/` or `platforms/` directory already exists under `{}`. Please delete them first.",
                root_for_checks.display()
            );
            std::process::exit(1);
        }

        Buck2Command::init().execute().unwrap_or_exit();
        let buck2_root = existing_root.unwrap_or_else(|| {
            get_buck2_root().unwrap_or_exit_ctx("failed to get Buck2 project root")
        });

        std::fs::create_dir_all(buck2_root.join(RUST_CRATES_ROOT))
            .unwrap_or_exit_ctx("failed to create third-party directory");
        let mut git_ignore = OpenOptions::new()
            .create(false)
            .append(true)
            .open(buck2_root.join(".gitignore"))
            .unwrap_or_exit();
        writeln!(git_ignore, "/buck-out").unwrap_or_exit();

        // Configure the buckal cell in .buckconfig
        init_buckal_cell(buck2_root.as_std_path()).unwrap_or_exit();

        extract_buck2_assets(buck2_root.as_std_path())
            .unwrap_or_exit_ctx("failed to extract buck2 assets");

        // Init cfg modifiers
        init_modifier(buck2_root.as_std_path()).unwrap_or_exit();
    }

    // Fetch latest bundles if requested
    if args.fetch {
        let cwd = std::env::current_dir().unwrap_or_exit();
        fetch_buckal_cell(&cwd).unwrap_or_exit();
    }

    // get cargo metadata and generate context
    let mut ctx = BuckalContext::new();
    ctx.no_merge = !args.merge;
    ctx.separate = args.separate;

    // Process the root node
    flush_root(&ctx);
    // Process dep nodes
    let last_cache = if args.no_cache || BuckalCache::load().is_err() {
        BuckalCache::new_empty()
    } else {
        BuckalCache::load().unwrap_or_exit_ctx("failed to load existing cache")
    };
    let new_cache = BuckalCache::new(&ctx.nodes_map, &ctx.workspace_root);
    let changes = new_cache.diff(&last_cache, &ctx.workspace_root);

    // Apply changes to BUCK files
    changes.apply(&ctx);

    // Flush the new cache
    new_cache.save();
}
