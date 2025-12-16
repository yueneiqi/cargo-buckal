use std::{
    collections::{BTreeSet, HashMap},
    process::Command,
    str::FromStr,
    sync::OnceLock,
};

use bitflags::bitflags;
use cargo_platform::{Cfg, Platform};

use crate::buckal_warn;

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

/// Tier1 host platforms used for cfg evaluation.
/// Ref: https://doc.rust-lang.org/nightly/rustc/platform-support.html#tier-1-with-host-tools
static SUPPORTED_TARGETS: &[(Os, &str)] = &[
    (Os::Macos, "aarch64-apple-darwin"),
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

/// Executes `rustc --print=cfg --target <triple>` to retrieve the cfg values for a given target triple.
///
/// This function is used to determine the platform-specific configuration flags that Cargo uses
/// to evaluate conditional compilation directives (cfg attributes) for different target platforms.
///
/// # Parameters
///
/// * `triple`: A target triple string (e.g., "x86_64-unknown-linux-gnu") that specifies the
///   platform for which to retrieve cfg values.
///
/// # Returns
///
/// * `Some(Vec<Cfg>)`: Returns the cfg values parsed from rustc's output when the command succeeds.
/// * `None`: Returns None when rustc execution fails, which can happen if:
///   - The target triple is not installed (e.g., missing rust target component)
///   - Rustc is not available in the system PATH
///   - The rustc command fails for any other reason
///
/// # Behavior
///
/// When this function returns `None`, the target triple is excluded from platform matching.
/// This is the expected behavior when a target is not installed, allowing the build system
/// to gracefully handle missing targets without failing the entire build process.
///
/// # Examples
///
/// ```
/// let cfgs = rustc_cfgs_for_triple("x86_64-unknown-linux-gnu");
/// if let Some(cfg_values) = cfgs {
///     // Use cfg_values for platform matching
/// } else {
///     // Target not available, skip platform matching for this triple
/// }
/// ```
fn rustc_cfgs_for_triple(triple: &'static str) -> Option<Vec<Cfg>> {
    match Command::new("rustc")
        .args(["--print=cfg", "--target", triple])
        .output()
    {
        Ok(output) if output.status.success() => {
            let cfgs: Vec<Cfg> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| Cfg::from_str(line).ok())
                .collect();
            Some(cfgs)
        }
        Ok(output) => {
            buckal_warn!(
                "Failed to run `rustc --print=cfg --target {}`: {}",
                triple,
                String::from_utf8_lossy(&output.stderr)
            );
            None
        }
        Err(error) => {
            buckal_warn!(
                "Failed to execute `rustc --print=cfg --target {}`: {}",
                triple,
                error
            );
            None
        }
    }
}

fn cfg_cache() -> &'static HashMap<&'static str, Vec<Cfg>> {
    CFG_CACHE.get_or_init(|| {
        // We spawn one thread per target triple. This is acceptable because:
        // 1. This initialization runs exactly once per program execution (OnceLock).
        // 2. The work is I/O-bound (waiting on rustc subprocess execution), not CPU-bound,
        //    so having more threads than cores improves throughput rather than causing
        //    contention - threads spend most of their time blocked on I/O.
        // 3. The bounded number of targets (tier-1 platforms) keeps thread count reasonable.
        let results = std::thread::scope(|scope| {
            let handles = SUPPORTED_TARGETS
                .iter()
                .map(|(_, triple)| {
                    let triple = *triple;
                    scope.spawn(move || (triple, rustc_cfgs_for_triple(triple)))
                })
                .collect::<Vec<_>>();

            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .expect("cfg_cache thread panicked while running rustc")
                })
                .collect::<Vec<_>>()
        });

        let mut map = HashMap::new();
        for (triple, cfgs) in results {
            if let Some(cfgs) = cfgs {
                map.insert(triple, cfgs);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    fn test_rustc_cfgs_for_triple_with_available_rustc() {
        let result = rustc_cfgs_for_triple("x86_64-unknown-linux-gnu");
        assert!(result.is_some());
        let cfgs = result.unwrap();

        // Verify essential cfg values for x86_64-unknown-linux-gnu
        assert!(!cfgs.is_empty(), "cfgs should not be empty");

        // Check for target_arch
        assert!(cfgs.iter().any(|cfg| cfg.to_string().contains("target_arch=\"x86_64\"")),
                "should contain target_arch=\"x86_64\": {:?}", cfgs);

        // Check for target_os
        assert!(cfgs.iter().any(|cfg| cfg.to_string().contains("target_os=\"linux\"")),
                "should contain target_os=\"linux\": {:?}", cfgs);

        // Check for target_env
        assert!(cfgs.iter().any(|cfg| cfg.to_string().contains("target_env=\"gnu\"")),
                "should contain target_env=\"gnu\": {:?}", cfgs);

        // Check for target_family
        assert!(cfgs.iter().any(|cfg| cfg.to_string().contains("target_family=\"unix\"")),
                "should contain target_family=\"unix\": {:?}", cfgs);

        // Check for unix flag (boolean cfg)
        assert!(cfgs.iter().any(|cfg| cfg.to_string() == "unix"),
                "should contain unix flag: {:?}", cfgs);

        // Check for debug_assertions flag (boolean cfg)
        assert!(cfgs.iter().any(|cfg| cfg.to_string() == "debug_assertions"),
                "should contain debug_assertions flag: {:?}", cfgs);

        // Verify minimum number of cfgs (should have at least the essential ones)
        assert!(cfgs.len() >= 10, "should have at least 10 cfg values, got {}: {:?}", cfgs.len(), cfgs);
    }

    #[test]
    fn test_cfg_parsing_direct() {
        // Test the cfg parsing logic directly by simulating rustc output
        // This tests the core logic without relying on external rustc execution
        let test_output = "target_arch=\"x86_64\"\ntarget_os=\"linux\"\ntarget_endian=\"little\"\n";
        let cfgs: Vec<Cfg> = String::from_utf8_lossy(test_output.as_bytes())
            .lines()
            .filter_map(|line| Cfg::from_str(line).ok())
            .collect();

        assert_eq!(cfgs.len(), 3);

        // Test that we can find specific cfgs by checking their string representation
        let target_arch_cfg = cfgs
            .iter()
            .find(|cfg| cfg.to_string().contains("target_arch"));
        assert!(target_arch_cfg.is_some());
        assert!(target_arch_cfg.unwrap().to_string().contains("x86_64"));

        let target_os_cfg = cfgs
            .iter()
            .find(|cfg| cfg.to_string().contains("target_os"));
        assert!(target_os_cfg.is_some());
        assert!(target_os_cfg.unwrap().to_string().contains("linux"));
    }

    #[test]
    fn test_cfg_parsing_boolean() {
        // Test parsing boolean cfg values
        let test_output = "debug_assertions\nverbose_errors\n";
        let cfgs: Vec<Cfg> = String::from_utf8_lossy(test_output.as_bytes())
            .lines()
            .filter_map(|line| Cfg::from_str(line).ok())
            .collect();

        assert_eq!(cfgs.len(), 2);
        assert!(cfgs.iter().any(|cfg| cfg.to_string() == "debug_assertions"));
        assert!(cfgs.iter().any(|cfg| cfg.to_string() == "verbose_errors"));
    }

    #[test]
    fn test_cfg_parsing_invalid_lines() {
        // Test that invalid cfg lines are filtered out
        let test_output = "target_arch=\"x86_64\"\ninvalid_line=bad_value\nrandom text\n";
        let cfgs: Vec<Cfg> = String::from_utf8_lossy(test_output.as_bytes())
            .lines()
            .filter_map(|line| Cfg::from_str(line).ok())
            .collect();

        // Only the valid cfg should be parsed (invalid_line=bad_value should fail)
        assert_eq!(cfgs.len(), 1);
        assert!(
            cfgs.iter()
                .any(|cfg| cfg.to_string().contains("target_arch"))
        );
    }

    #[test]
    fn test_platform_mask_operations() {
        // Test PlatformMask operations
        let mask = PlatformMask::WINDOWS | PlatformMask::LINUX;
        assert!(mask.contains(PlatformMask::WINDOWS));
        assert!(!mask.contains(PlatformMask::MACOS));
        assert!(mask.contains(PlatformMask::LINUX));

        let oses = mask.to_oses();
        let mut expected = BTreeSet::new();
        expected.insert(Os::Windows);
        expected.insert(Os::Linux);
        assert_eq!(oses, expected);
    }

    #[test]
    fn test_os_buck_labels() {
        // Test Os enum methods
        assert_eq!(Os::Windows.buck_label(), "prelude//os/constraints:windows");
        assert_eq!(Os::Macos.buck_label(), "prelude//os/constraints:macos");
        assert_eq!(Os::Linux.buck_label(), "prelude//os/constraints:linux");

        assert_eq!(Os::Windows.key(), "windows");
        assert_eq!(Os::Macos.key(), "macos");
        assert_eq!(Os::Linux.key(), "linux");
    }

    #[test]
    fn test_lookup_platforms() {
        // Test package platform lookup
        let windows_pkgs = lookup_platforms("windows-future").unwrap();
        let mut expected = BTreeSet::new();
        expected.insert(Os::Windows);
        assert_eq!(windows_pkgs, expected);

        let macos_pkgs = lookup_platforms("system-configuration").unwrap();
        let mut expected = BTreeSet::new();
        expected.insert(Os::Macos);
        assert_eq!(macos_pkgs, expected);

        // Test unknown package returns None
        assert!(lookup_platforms("unknown-package").is_none());
    }

    #[test]
    fn test_buck_labels_utility() {
        // Test the buck_labels utility function
        let mut oses = BTreeSet::new();
        oses.insert(Os::Windows);
        oses.insert(Os::Linux);

        let labels = buck_labels(&oses);
        let mut expected = BTreeSet::new();
        expected.insert("prelude//os/constraints:windows".to_string());
        expected.insert("prelude//os/constraints:linux".to_string());
        assert_eq!(labels, expected);
    }

    #[test]
    fn test_supported_targets() {
        // Test that supported targets are defined and non-empty
        assert!(!SUPPORTED_TARGETS.is_empty());

        // Test that each supported target has a valid OS and triple
        for (os, triple) in SUPPORTED_TARGETS {
            assert!(matches!(os, Os::Windows | Os::Macos | Os::Linux));
            assert!(!triple.is_empty());
        }
    }
}
