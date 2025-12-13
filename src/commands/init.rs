use std::{
    fs::OpenOptions,
    io::Write,
    process::{Command, Stdio, exit},
};

use clap::Parser;

use crate::{
    RUST_CRATES_ROOT,
    buck2::Buck2Command,
    buckal_error, buckal_log, buckal_note,
    bundles::{init_buckal_cell, init_modifier},
    utils::{UnwrapOrExit, ensure_prerequisites},
};

#[derive(Parser, Debug)]
pub struct InitArgs {
    #[arg(long, default_value = "false")]
    pub bin: bool,
    #[arg(long, default_value = "false")]
    pub lib: bool,
    #[arg(long)]
    pub edition: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, default_value = "false", conflicts_with_all = ["bin", "lib", "edition", "name"])]
    pub repo: bool,
    #[arg(long, default_value = "false", conflicts_with = "repo")]
    pub lite: bool,
}

pub fn execute(args: &InitArgs) {
    // Ensure all prerequisites are installed before proceeding
    ensure_prerequisites().unwrap_or_exit();

    // Use `cargo new` to initialize the directory
    let mut cargo_cmd = Command::new("cargo");
    cargo_cmd.arg("init");
    if args.bin {
        cargo_cmd.arg("--bin");
    }
    if args.lib {
        cargo_cmd.arg("--lib");
    }
    if let Some(edition) = &args.edition {
        cargo_cmd.arg("--edition").arg(edition);
    }
    if let Some(name) = &args.name {
        cargo_cmd.arg("--name").arg(name);
    }

    // Suppress output if `--repo` is set
    if args.repo {
        cargo_cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }

    // execute the cargo command
    let status = cargo_cmd
        .status()
        .unwrap_or_exit_ctx("failed to execute `cargo init`");
    if !status.success() {
        buckal_error!("failed to initialize directory");
        exit(1);
    }

    // If `--repo` is set, remove the generated `src` directory and `Cargo.toml`
    if args.repo {
        buckal_log!("Creating", "buck2 repository");
        std::fs::remove_dir_all("./src").unwrap_or_exit();
        std::fs::remove_file("./Cargo.toml").unwrap_or_exit();
        buckal_note!(
            "You should manually configure a Cargo workspace before running `cargo buckal new <path>` to create packages."
        );
    }

    if args.repo || args.lite {
        // Init a new buck2 repo
        Buck2Command::init().execute().unwrap_or_exit();
        std::fs::create_dir_all(RUST_CRATES_ROOT)
            .unwrap_or_exit_ctx("failed to create third-party directory");
        let mut git_ignore = OpenOptions::new()
            .create(false)
            .append(true)
            .open(".gitignore")
            .unwrap_or_exit();
        writeln!(git_ignore, "/buck-out").unwrap_or_exit();

        // Configure the buckal cell in .buckconfig
        let cwd = std::env::current_dir().unwrap_or_exit();
        init_buckal_cell(&cwd).unwrap_or_exit();

        // Init cfg modifiers
        init_modifier(&cwd).unwrap_or_exit();
    } else {
        // Create a new buck2 cell
        let _buck =
            std::fs::File::create("BUCK").unwrap_or_exit_ctx("failed to create `BUCK` file");
    }

    if args.repo {
        buckal_note!(
            "You should manually configure a Cargo workspace before running `cargo buckal new <path>` to create packages."
        );
    }
}
