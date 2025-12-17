use std::{borrow::Cow, vec};

use starlark_syntax::codemap::Spanned;
use starlark_syntax::syntax::ast::{
    AstNoPayload, ArgumentP, AstExpr, AstLiteral, AstStmt, ExprP, Stmt,
};
use starlark_syntax::syntax::module::AstModuleFields;
use starlark_syntax::syntax::{AstModule, Dialect};

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
        buck_content = apply_rustc_flags_patch_to_content(&buck_content, &bin_name, &select_expr);
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

fn apply_rustc_flags_patch_to_content(
    buck_content: &str,
    bin_name: &str,
    select_expr: &str,
) -> String {
    // Parse the Starlark content into an AST
    let ast = match AstModule::parse("BUCK", buck_content.to_owned(), &Dialect::Extended) {
        Ok(ast) => ast,
        Err(_) => return buck_content.to_owned(),
    };

    // Find the insertion point by walking the AST
    let insert_pos = match find_rustc_flags_end_in_rust_binary(ast.statement(), bin_name) {
        Some(pos) => pos,
        None => return buck_content.to_owned(),
    };

    // Insert the select expression at the found position
    let mut out = String::with_capacity(buck_content.len() + select_expr.len() + 4);
    out.push_str(&buck_content[..insert_pos]);
    out.push_str(" + ");
    out.push_str(select_expr);
    out.push_str(&buck_content[insert_pos..]);
    out
}

/// Walk the AST to find a `rust_binary` call with the given name and return the
/// byte position just after the closing `]` of its `rustc_flags` list.
fn find_rustc_flags_end_in_rust_binary(stmt: &AstStmt, target_name: &str) -> Option<usize> {
    match &stmt.node {
        Stmt::Statements(stmts) => {
            for s in stmts {
                if let Some(pos) = find_rustc_flags_end_in_rust_binary(s, target_name) {
                    return Some(pos);
                }
            }
            None
        }
        Stmt::Expression(expr) => find_in_expr(expr, target_name),
        _ => None,
    }
}

fn find_in_expr(expr: &AstExpr, target_name: &str) -> Option<usize> {
    if let ExprP::Call(callee, args) = &expr.node {
        // Check if this is a call to `rust_binary`
        if let ExprP::Identifier(ident) = &callee.node
            && ident.node.ident == "rust_binary"
        {
            return find_rustc_flags_in_call(&args.args, target_name);
        }
    }
    None
}

fn find_rustc_flags_in_call(
    args: &[Spanned<ArgumentP<AstNoPayload>>],
    target_name: &str,
) -> Option<usize> {
    // First, check if the `name` argument matches
    let mut name_matches = false;
    let mut rustc_flags_end: Option<usize> = None;

    for arg in args {
        if let ArgumentP::Named(name_spanned, value) = &arg.node {
            let arg_name = &name_spanned.node;
            if arg_name == "name" {
                // Check if the value is a string literal matching our target
                if let ExprP::Literal(AstLiteral::String(s)) = &value.node
                    && s.node == target_name
                {
                    name_matches = true;
                }
            } else if arg_name == "rustc_flags" {
                // Get the end position of the rustc_flags value (should be a list)
                if let ExprP::List(_) = &value.node {
                    rustc_flags_end = Some(value.span.end().get() as usize);
                }
            }
        }
    }

    if name_matches {
        rustc_flags_end
    } else {
        None
    }
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
    fn apply_rustc_flags_patch_to_content_patches_named_binary_only() {
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

        let patched = apply_rustc_flags_patch_to_content(input, "bin", "select({\"DEFAULT\": []})");
        assert_eq!(patched, expected);
    }

    #[test]
    fn apply_rustc_flags_patch_to_content_does_not_touch_other_binaries() {
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

        let patched = apply_rustc_flags_patch_to_content(input, "b", "select({\"DEFAULT\": []})");
        assert_eq!(patched, expected);
    }
}
