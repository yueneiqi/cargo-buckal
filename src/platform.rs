use std::{
    collections::BTreeSet, collections::HashMap, process::Command, str::FromStr, sync::OnceLock,
};

use bitflags::bitflags;
use cargo_platform::{Cfg, Platform};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

impl Os {
    pub fn buck_label(self) -> &'static str {
        match self {
            // Use canonical prelude constraint values so selects work with
            // platform definitions like `prelude//os/constraints:linux`.
            Os::Windows => "prelude//os/constraints:windows",
            Os::Macos => "prelude//os/constraints:macos",
            Os::Linux => "prelude//os/constraints:linux",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Os::Windows => "windows",
            Os::Macos => "macos",
            Os::Linux => "linux",
        }
    }
}

/// Tier1 host platforms (plus explicit x86_64-apple-darwin) used for cfg evaluation.
static SUPPORTED_TARGETS: &[(Os, &str)] = &[
    (Os::Macos, "aarch64-apple-darwin"),
    (Os::Macos, "x86_64-apple-darwin"),
    (Os::Windows, "aarch64-pc-windows-msvc"),
    (Os::Windows, "x86_64-pc-windows-msvc"),
    (Os::Windows, "x86_64-pc-windows-gnu"),
    (Os::Windows, "i686-pc-windows-msvc"),
    (Os::Linux, "aarch64-unknown-linux-gnu"),
    (Os::Linux, "x86_64-unknown-linux-gnu"),
    (Os::Linux, "i686-unknown-linux-gnu"),
];

/// Cache of `rustc --print=cfg --target <triple>` output for supported triples.
static CFG_CACHE: OnceLock<HashMap<&'static str, Vec<Cfg>>> = OnceLock::new();

fn cfg_cache() -> &'static HashMap<&'static str, Vec<Cfg>> {
	    CFG_CACHE.get_or_init(|| {
	        let mut map = HashMap::new();
	        for (_, triple) in SUPPORTED_TARGETS {
	            if let Ok(output) = Command::new("rustc")
	                .args(["--print=cfg", "--target", triple])
	                .output()
	                && output.status.success()
	            {
	                let cfgs: Vec<Cfg> = String::from_utf8_lossy(&output.stdout)
	                    .lines()
	                    .filter_map(|line| Cfg::from_str(line).ok())
	                    .collect();
	                map.insert(*triple, cfgs);
	            }
	        }
	        map
	    })
	}

pub fn buck_labels(oses: &BTreeSet<Os>) -> BTreeSet<String> {
    oses.iter().map(|os| os.buck_label().to_string()).collect()
}

pub fn oses_from_platform(platform: &Platform) -> BTreeSet<Os> {
    let cfgs = cfg_cache();
    SUPPORTED_TARGETS
        .iter()
        .filter_map(|(os, triple)| {
            cfgs.get(triple).and_then(|cfgs| {
                if platform.matches(triple, cfgs) {
                    Some(*os)
                } else {
                    None
                }
            })
        })
        .collect()
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct PlatformMask: u32 {
        const WINDOWS = 0b0001;
        const MACOS   = 0b0010;
        const LINUX   = 0b0100;
    }
}

impl PlatformMask {
    pub fn to_oses(self) -> BTreeSet<Os> {
        let mut set = BTreeSet::new();
        if self.contains(Self::WINDOWS) {
            set.insert(Os::Windows);
        }
        if self.contains(Self::MACOS) {
            set.insert(Os::Macos);
        }
        if self.contains(Self::LINUX) {
            set.insert(Os::Linux);
        }
        set
    }
}

static PACKAGE_PLATFORMS: phf::Map<&'static str, PlatformMask> = phf::phf_map! {
    "hyper-named-pipe" => PlatformMask::WINDOWS,
    "system-configuration" => PlatformMask::MACOS,
    "windows-future" => PlatformMask::WINDOWS,
    "windows" => PlatformMask::WINDOWS,
    "winreg" => PlatformMask::WINDOWS,
};

pub fn lookup_platforms(package_name: &str) -> Option<BTreeSet<Os>> {
    PACKAGE_PLATFORMS
        .get(package_name)
        .map(|mask| mask.to_oses())
}
