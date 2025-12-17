use std::{borrow::Cow, vec};

use crate::{RUST_CRATES_ROOT, context::BuckalContext};

#[derive(Default)]
struct WindowsImportLibFlags {
    gnu: Vec<String>,
    msvc_x86_64: Vec<String>,
    msvc_i686: Vec<String>,
    msvc_aarch64: Vec<String>,
}

pub(super) fn patch_root_windows_rustc_flags(
    mut buck_content: String,
    ctx: &BuckalContext,
) -> String {
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

    let push_build_script_rustc_flags = |package_name: &str, out: &mut Vec<String>| {
        let mut matches: Vec<_> = ctx
            .packages_map
            .values()
            .filter(|p| p.name.to_string() == package_name)
            .collect();
        matches.sort_by(|a, b| a.version.cmp(&b.version));
        for package in matches {
            let pkg_name = package.name.to_string();
            out.push(format!(
                "@$(location //{}/{}/{}:{}-build-script-run[rustc_flags])",
                RUST_CRATES_ROOT, pkg_name, package.version, pkg_name
            ));
        }
    };

    // GNU targets.
    push_build_script_rustc_flags("windows_x86_64_gnu", &mut flags.gnu);
    push_build_script_rustc_flags("winapi-x86_64-pc-windows-gnu", &mut flags.gnu);

    // MSVC targets (per CPU).
    push_build_script_rustc_flags("windows_x86_64_msvc", &mut flags.msvc_x86_64);
    push_build_script_rustc_flags("windows_i686_msvc", &mut flags.msvc_i686);
    push_build_script_rustc_flags("windows_aarch64_msvc", &mut flags.msvc_aarch64);

    flags
}

fn render_windows_rustc_flags_select(flags: &WindowsImportLibFlags) -> String {
    const CONSTRAINT_WINDOWS: &str = "prelude//os/constraints:windows";
    const CONSTRAINT_ABI_GNU: &str = "prelude//abi/constraints:gnu";
    const CONSTRAINT_ABI_MSVC: &str = "prelude//abi/constraints:msvc";
    const CONSTRAINT_CPU_ARM64: &str = "prelude//cpu/constraints:arm64";
    const CONSTRAINT_CPU_X86_32: &str = "prelude//cpu/constraints:x86_32";
    const SELECT_DEFAULT: &str = "DEFAULT";

    if flags.gnu.is_empty()
        && flags.msvc_x86_64.is_empty()
        && flags.msvc_i686.is_empty()
        && flags.msvc_aarch64.is_empty()
    {
        return String::new();
    }

    #[derive(Clone, Debug)]
    enum BuckExpr<'a> {
        Str(Cow<'a, str>),
        List {
            items: Vec<BuckExpr<'a>>,
            multiline: bool,
        },
        Select(Vec<(Cow<'a, str>, BuckExpr<'a>)>),
    }

    fn write_indent(out: &mut String, spaces: usize) {
        for _ in 0..spaces {
            out.push(' ');
        }
    }

    fn write_string_literal(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push(c),
            }
        }
        out.push('"');
    }

    impl<'a> BuckExpr<'a> {
        fn string_list(items: &'a [String]) -> Self {
            Self::List {
                items: items
                    .iter()
                    .map(|s| Self::Str(Cow::Borrowed(s.as_str())))
                    .collect(),
                multiline: true,
            }
        }

        fn empty_inline_list() -> Self {
            Self::List {
                items: vec![],
                multiline: false,
            }
        }

        fn write_inline(&self, out: &mut String, base_indent: usize) {
            match self {
                Self::Str(s) => write_string_literal(out, s),
                Self::List { items, multiline } => {
                    if *multiline {
                        out.push_str("[\n");
                        for item in items {
                            write_indent(out, base_indent + 4);
                            item.write_inline(out, base_indent + 4);
                            out.push_str(",\n");
                        }
                        write_indent(out, base_indent);
                        out.push(']');
                        return;
                    }

                    out.push('[');
                    for (idx, item) in items.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        item.write_inline(out, base_indent);
                    }
                    out.push(']');
                }
                Self::Select(entries) => {
                    out.push_str("select({\n");
                    for (k, v) in entries {
                        write_indent(out, base_indent + 4);
                        write_string_literal(out, k);
                        out.push_str(": ");
                        v.write_inline(out, base_indent + 4);
                        out.push_str(",\n");
                    }
                    write_indent(out, base_indent);
                    out.push_str("})");
                }
            }
        }
    }

    fn msvc_cpu_select(flags: &WindowsImportLibFlags) -> BuckExpr<'_> {
        BuckExpr::Select(vec![
            (
                Cow::Borrowed(CONSTRAINT_CPU_ARM64),
                BuckExpr::string_list(&flags.msvc_aarch64),
            ),
            (
                Cow::Borrowed(CONSTRAINT_CPU_X86_32),
                BuckExpr::string_list(&flags.msvc_i686),
            ),
            (
                Cow::Borrowed(SELECT_DEFAULT),
                BuckExpr::string_list(&flags.msvc_x86_64),
            ),
        ])
    }

    let windows_select = BuckExpr::Select(vec![
        (
            Cow::Borrowed(CONSTRAINT_ABI_GNU),
            BuckExpr::string_list(&flags.gnu),
        ),
        (Cow::Borrowed(CONSTRAINT_ABI_MSVC), msvc_cpu_select(flags)),
        (Cow::Borrowed(SELECT_DEFAULT), msvc_cpu_select(flags)),
    ]);

    let select_expr = BuckExpr::Select(vec![
        (Cow::Borrowed(CONSTRAINT_WINDOWS), windows_select),
        (Cow::Borrowed(SELECT_DEFAULT), BuckExpr::empty_inline_list()),
    ]);

    let mut out = String::new();
    // The expression is appended inline after `] +`, but we want the body to be indented as if it
    // started at the `rustc_flags` attribute's indentation level (4 spaces).
    select_expr.write_inline(&mut out, 4);
    out
}

fn patch_rust_binary_rustc_flags(buck_content: &str, bin_name: &str, select_expr: &str) -> String {
    fn find_rule_end(haystack: &str, start: usize) -> Option<usize> {
        // Find a closing paren on its own line (column 0), which is how serde_starlark ends rules.
        // Return the index just after the ')'.
        let mut search_from = start;
        while let Some(rel) = haystack[search_from..].find("\n)") {
            let close_paren = search_from + rel + 1;
            let next = close_paren + 1;
            if next == haystack.len() || haystack.as_bytes()[next] == b'\n' {
                return Some(next);
            }
            search_from = next;
        }
        None
    }

    let name_marker = format!("    name = \"{bin_name}\",");
    let rustc_flags_marker = "    rustc_flags = [";

    let mut search_from = 0usize;
    while let Some(block_start_rel) = buck_content[search_from..].find("rust_binary(\n") {
        let block_start = search_from + block_start_rel;
        let Some(block_end) = find_rule_end(buck_content, block_start) else {
            break;
        };

        let block = &buck_content[block_start..block_end];
        if !block.contains(&name_marker) {
            search_from = block_end;
            continue;
        }

        let Some(rustc_flags_rel) = block.find(rustc_flags_marker) else {
            return buck_content.to_owned();
        };
        let rustc_flags_pos = block_start + rustc_flags_rel;

        let after_rustc_flags = rustc_flags_pos + rustc_flags_marker.len();
        let Some(list_end_rel) = buck_content[after_rustc_flags..block_end].find("\n    ],\n")
        else {
            return buck_content.to_owned();
        };
        let list_end = after_rustc_flags + list_end_rel + "\n    ]".len();

        let mut out = String::with_capacity(buck_content.len() + select_expr.len() + 64);
        out.push_str(&buck_content[..list_end]);
        out.push_str(" + ");
        out.push_str(select_expr);
        out.push_str(&buck_content[list_end..]);
        return out;
    }

    buck_content.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;

    #[test]
    fn render_windows_rustc_flags_select_empty() {
        let flags = WindowsImportLibFlags::default();
        assert_eq!(render_windows_rustc_flags_select(&flags), "");
    }

    #[test]
    fn render_windows_rustc_flags_select_structured_output() {
        let flags = WindowsImportLibFlags {
            gnu: vec!["@gnu1".to_owned(), "@gnu2".to_owned()],
            msvc_x86_64: vec!["@msvc64".to_owned()],
            msvc_i686: vec!["@msvc32".to_owned()],
            msvc_aarch64: vec!["@msvcarm".to_owned()],
        };

        let rendered = render_windows_rustc_flags_select(&flags);

        let expected = indoc! {r#"
            select({
                    "prelude//os/constraints:windows": select({
                        "prelude//abi/constraints:gnu": [
                            "@gnu1",
                            "@gnu2",
                        ],
                        "prelude//abi/constraints:msvc": select({
                            "prelude//cpu/constraints:arm64": [
                                "@msvcarm",
                            ],
                            "prelude//cpu/constraints:x86_32": [
                                "@msvc32",
                            ],
                            "DEFAULT": [
                                "@msvc64",
                            ],
                        }),
                        "DEFAULT": select({
                            "prelude//cpu/constraints:arm64": [
                                "@msvcarm",
                            ],
                            "prelude//cpu/constraints:x86_32": [
                                "@msvc32",
                            ],
                            "DEFAULT": [
                                "@msvc64",
                            ],
                        }),
                    }),
                    "DEFAULT": [],
                })"#};

        assert_eq!(rendered, expected);
    }

    #[test]
    fn patch_rust_binary_rustc_flags_patches_named_binary_only() {
        let input = indoc! {r#"
            rust_library(
                name = "bin",
                rustc_flags = [
                    "libflag",
                ],
            )

            rust_binary(
                name = "bin",
                rustc_flags = [
                    "binflag",
                ],
            )
            "#};

        let expected = indoc! {r#"
            rust_library(
                name = "bin",
                rustc_flags = [
                    "libflag",
                ],
            )

            rust_binary(
                name = "bin",
                rustc_flags = [
                    "binflag",
                ] + select({"DEFAULT": []}),
            )
            "#};

        let patched = patch_rust_binary_rustc_flags(input, "bin", "select({\"DEFAULT\": []})");
        assert_eq!(patched, expected);
    }

    #[test]
    fn patch_rust_binary_rustc_flags_does_not_touch_other_binaries() {
        let input = indoc! {r#"
            rust_binary(
                name = "a",
                rustc_flags = [
                    "aflag",
                ],
            )

            rust_binary(
                name = "b",
                rustc_flags = [
                    "bflag",
                ],
            )
            "#};

        let expected = indoc! {r#"
            rust_binary(
                name = "a",
                rustc_flags = [
                    "aflag",
                ],
            )

            rust_binary(
                name = "b",
                rustc_flags = [
                    "bflag",
                ] + select({"DEFAULT": []}),
            )
            "#};

        let patched = patch_rust_binary_rustc_flags(input, "b", "select({\"DEFAULT\": []})");
        assert_eq!(patched, expected);
    }
}
