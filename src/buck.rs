use std::collections::{BTreeMap as Map, BTreeSet as Set};
use std::ffi::CString;

use cargo_metadata::camino::Utf8PathBuf;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};
use pyo3_ffi::c_str;
use serde::ser::{Serialize, SerializeStruct, SerializeTupleStruct, Serializer};
use serde_derive::Serialize;

#[derive(Serialize, Debug)]
#[serde(untagged)]
pub enum Rule {
    Load(Load),
    HttpArchive(HttpArchive),
    FileGroup(FileGroup),
    CargoManifest(CargoManifest),
    RustLibrary(RustLibrary),
    RustBinary(RustBinary),
    RustTest(RustTest),
    BuildscriptRun(BuildscriptRun),
}
#[derive(Serialize, Debug)]
#[serde(rename = "alias")]
pub struct Alias {
    pub name: String,
    pub actual: String,
    pub visibility: Set<String>,
}
impl Rule {
    pub fn as_rust_rule_mut(&mut self) -> Option<&mut dyn RustRule> {
        match self {
            Rule::RustLibrary(inner) => Some(inner),
            Rule::RustBinary(inner) => Some(inner),
            _ => None,
        }
    }
}

pub trait RustRule {
    fn deps_mut(&mut self) -> &mut Set<String>;
    fn os_deps_mut(&mut self) -> &mut Map<String, Set<String>>;
    fn rustc_flags_mut(&mut self) -> &mut Set<String>;
    fn env_mut(&mut self) -> &mut Map<String, String>;
    fn named_deps_mut(&mut self) -> &mut Map<String, String>;
    fn os_named_deps_mut(&mut self) -> &mut Map<String, Map<String, String>>;
}

#[derive(PartialEq, Clone, Copy)]
pub enum CargoTargetKind {
    Lib,
    Bin,
    CustomBuild,
    Test,
}

#[derive(Debug)]
pub struct Load {
    pub bzl: String,
    pub items: Set<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "http_archive")]
pub struct HttpArchive {
    pub name: String,
    pub urls: Set<String>,
    pub sha256: String,
    #[serde(rename = "type")]
    pub _type: String,
    pub strip_prefix: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "cargo_manifest")]
pub struct CargoManifest {
    pub name: String,
    pub vendor: String,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "rust_library")]
pub struct RustLibrary {
    pub name: String,
    pub srcs: Set<String>,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub crate_root: String,
    pub edition: String,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub target_compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub exec_compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub env: Map<String, String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub features: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub rustc_flags: Set<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proc_macro: Option<bool>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub named_deps: Map<String, String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub os_named_deps: Map<String, Map<String, String>>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub os_deps: Map<String, Set<String>>,
    pub visibility: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub deps: Set<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "rust_binary")]
pub struct RustBinary {
    pub name: String,
    pub srcs: Set<String>,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub crate_root: String,
    pub edition: String,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub target_compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub exec_compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub env: Map<String, String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub features: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub rustc_flags: Set<String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub named_deps: Map<String, String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub os_named_deps: Map<String, Map<String, String>>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub os_deps: Map<String, Set<String>>,
    pub visibility: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub deps: Set<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "rust_test")]
pub struct RustTest {
    pub name: String,
    pub srcs: Set<String>,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub crate_root: String,
    pub edition: String,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub target_compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub exec_compatible_with: Set<String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub env: Map<String, String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub features: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub rustc_flags: Set<String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub named_deps: Map<String, String>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub os_named_deps: Map<String, Map<String, String>>,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub os_deps: Map<String, Set<String>>,
    pub visibility: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub deps: Set<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "buildscript_run")]
pub struct BuildscriptRun {
    pub name: String,
    pub package_name: String,
    pub buildscript_rule: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub env: Map<String, String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub env_srcs: Set<String>,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub features: Set<String>,
    pub version: String,
    pub manifest_dir: String,
    #[serde(skip_serializing_if = "Set::is_empty")]
    pub visibility: Set<String>,
}

#[derive(Default, Debug)]
pub struct Glob {
    pub include: Set<String>,
    pub exclude: Set<String>,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename = "filegroup")]
pub struct FileGroup {
    pub name: String,
    pub srcs: Glob,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
}

impl Serialize for Load {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_tuple_struct("load", 0)?;
        s.serialize_field(&self.bzl)?;
        for item in &self.items {
            s.serialize_field(item)?;
        }
        s.end()
    }
}

impl Serialize for Glob {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.exclude.is_empty() {
            serializer.serialize_newtype_struct("glob", &self.include)
        } else {
            let mut s = serializer.serialize_struct("glob", 2)?;
            s.serialize_field("include", &self.include)?;
            s.serialize_field("exclude", &self.exclude)?;
            s.end()
        }
    }
}

impl Glob {
    fn from_py_tuple(tuple: &Bound<'_, PyTuple>) -> PyResult<Self> {
        let func_binding = tuple.get_item(0).unwrap();
        let func = func_binding.downcast::<PyString>().unwrap();
        assert_eq!(func.to_str().unwrap(), "glob");
        let args_binding = tuple.get_item(1).unwrap();
        let args = args_binding.downcast::<PyTuple>().unwrap();
        if args.len() > 1 {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "glob only supports one positional argument",
            ))
        } else if args.len() == 1 {
            let include_vec: Vec<String> = args
                .get_item(0)
                .expect("Expected one positional argument")
                .extract()
                .ok()
                .unwrap_or_default();
            let include: Set<String> = include_vec.into_iter().collect();
            Ok(Glob {
                include,
                exclude: Set::new(),
            })
        } else {
            let kwargs_binding = tuple.get_item(2).unwrap();
            let kwargs = kwargs_binding.downcast::<PyDict>().unwrap();
            let include_vec: Vec<String> = get_arg(kwargs, "include");
            let include: Set<String> = include_vec.into_iter().collect();
            let exclude_vec: Vec<String> = get_arg(kwargs, "exclude");
            let exclude: Set<String> = exclude_vec.into_iter().collect();
            Ok(Glob { include, exclude })
        }
    }
}

impl RustRule for RustLibrary {
    fn deps_mut(&mut self) -> &mut Set<String> {
        &mut self.deps
    }

    fn os_deps_mut(&mut self) -> &mut Map<String, Set<String>> {
        &mut self.os_deps
    }

    fn rustc_flags_mut(&mut self) -> &mut Set<String> {
        &mut self.rustc_flags
    }

    fn env_mut(&mut self) -> &mut Map<String, String> {
        &mut self.env
    }

    fn named_deps_mut(&mut self) -> &mut Map<String, String> {
        &mut self.named_deps
    }

    fn os_named_deps_mut(&mut self) -> &mut Map<String, Map<String, String>> {
        &mut self.os_named_deps
    }
}

impl RustRule for RustBinary {
    fn deps_mut(&mut self) -> &mut Set<String> {
        &mut self.deps
    }

    fn os_deps_mut(&mut self) -> &mut Map<String, Set<String>> {
        &mut self.os_deps
    }

    fn rustc_flags_mut(&mut self) -> &mut Set<String> {
        &mut self.rustc_flags
    }

    fn env_mut(&mut self) -> &mut Map<String, String> {
        &mut self.env
    }

    fn named_deps_mut(&mut self) -> &mut Map<String, String> {
        &mut self.named_deps
    }

    fn os_named_deps_mut(&mut self) -> &mut Map<String, Map<String, String>> {
        &mut self.os_named_deps
    }
}

impl RustRule for RustTest {
    fn deps_mut(&mut self) -> &mut Set<String> {
        &mut self.deps
    }

    fn os_deps_mut(&mut self) -> &mut Map<String, Set<String>> {
        &mut self.os_deps
    }

    fn rustc_flags_mut(&mut self) -> &mut Set<String> {
        &mut self.rustc_flags
    }

    fn env_mut(&mut self) -> &mut Map<String, String> {
        &mut self.env
    }

    fn named_deps_mut(&mut self) -> &mut Map<String, String> {
        &mut self.named_deps
    }

    fn os_named_deps_mut(&mut self) -> &mut Map<String, Map<String, String>> {
        &mut self.os_named_deps
    }
}

macro_rules! extract_set {
    ($kwargs:expr, $key:literal) => {{
        let vec: Vec<String> = get_arg($kwargs, $key);
        vec.into_iter().collect::<Set<String>>()
    }};
}

fn patch_map<K, V>(dst: &mut Map<K, V>, src: &Map<K, V>)
where
    K: Clone + Ord,
    V: Clone,
{
    for (k, v) in src {
        dst.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

fn patch_set<T>(dst: &mut Set<T>, src: &Set<T>)
where
    T: Clone + Ord,
{
    let to_add: Vec<_> = src.difference(dst).cloned().collect();
    dst.extend(to_add);
}

struct DepFieldsMut<'a> {
    deps: &'a mut Set<String>,
    os_deps: &'a mut Map<String, Set<String>>,
    named_deps: &'a mut Map<String, String>,
    os_named_deps: &'a mut Map<String, Map<String, String>>,
}

struct DepFieldsRef<'a> {
    deps: &'a Set<String>,
    os_deps: &'a Map<String, Set<String>>,
    named_deps: &'a Map<String, String>,
    os_named_deps: &'a Map<String, Map<String, String>>,
}

fn patch_deps_fields(patch_fields: &Set<String>, dst: &mut DepFieldsMut, src: &DepFieldsRef) {
    if patch_fields.contains("deps") {
        patch_set(dst.deps, src.deps);
    }

    if patch_fields.contains("os_deps") {
        for (plat, deps) in src.os_deps {
            patch_set(dst.os_deps.entry(plat.clone()).or_default(), deps);
        }
    }

    if patch_fields.contains("named_deps") {
        patch_map(dst.named_deps, src.named_deps);
    }

    if patch_fields.contains("os_named_deps") {
        for (alias, plat_map) in src.os_named_deps {
            let entry = dst.os_named_deps.entry(alias.clone()).or_default();
            patch_map(entry, plat_map);
        }
    }
}

impl RustLibrary {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let srcs: Set<String> = extract_set!(kwargs, "srcs");
        let crate_name: String = get_arg(kwargs, "crate");
        let crate_root: String = get_arg(kwargs, "crate_root");
        let edition: String = get_arg(kwargs, "edition");
        let target_compatible_with: Set<String> = extract_set!(kwargs, "target_compatible_with");
        let compatible_with: Set<String> = extract_set!(kwargs, "compatible_with");
        let exec_compatible_with: Set<String> = extract_set!(kwargs, "exec_compatible_with");
        let env: Map<String, String> = get_arg(kwargs, "env");
        let features: Set<String> = extract_set!(kwargs, "features");
        let rustc_flags: Set<String> = extract_set!(kwargs, "rustc_flags");
        let proc_macro: Option<bool> = get_arg(kwargs, "proc_macro");
        let named_deps: Map<String, String> = get_arg(kwargs, "named_deps");
        let os_named_deps: Map<String, Map<String, String>> = get_arg(kwargs, "os_named_deps");
        let os_deps: Map<String, Set<String>> = get_arg(kwargs, "os_deps");
        let visibility: Set<String> = extract_set!(kwargs, "visibility");
        let deps: Set<String> = extract_set!(kwargs, "deps");
        Ok(RustLibrary {
            name,
            srcs,
            crate_name,
            crate_root,
            edition,
            target_compatible_with,
            compatible_with,
            exec_compatible_with,
            env,
            features,
            rustc_flags,
            proc_macro,
            named_deps,
            os_named_deps,
            os_deps,
            visibility,
            deps,
        })
    }

    fn patch_from(&mut self, other: &RustLibrary, patch_fields: &Set<String>) {
        // Patch target_compatible_with set
        if patch_fields.contains("target_compatible_with") {
            patch_set(
                &mut self.target_compatible_with,
                &other.target_compatible_with,
            );
        }
        // Patch compatible_with set
        if patch_fields.contains("compatible_with") {
            patch_set(&mut self.compatible_with, &other.compatible_with);
        }
        // Patch exec_compatible_with set
        if patch_fields.contains("exec_compatible_with") {
            patch_set(&mut self.exec_compatible_with, &other.exec_compatible_with);
        }
        // Patch env map
        if patch_fields.contains("env") {
            patch_map(&mut self.env, &other.env);
        }
        // Patch features set
        if patch_fields.contains("features") {
            patch_set(&mut self.features, &other.features);
        }
        // Patch rustc_flags set
        if patch_fields.contains("rustc_flags") {
            patch_set(&mut self.rustc_flags, &other.rustc_flags);
        }
        // Patch visibility set
        if patch_fields.contains("visibility") {
            patch_set(&mut self.visibility, &other.visibility);
        }

        let mut dst = DepFieldsMut {
            deps: &mut self.deps,
            os_deps: &mut self.os_deps,
            named_deps: &mut self.named_deps,
            os_named_deps: &mut self.os_named_deps,
        };
        let src = DepFieldsRef {
            deps: &other.deps,
            os_deps: &other.os_deps,
            named_deps: &other.named_deps,
            os_named_deps: &other.os_named_deps,
        };
        patch_deps_fields(patch_fields, &mut dst, &src);
    }
}

impl RustBinary {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let srcs: Set<String> = extract_set!(kwargs, "srcs");
        let crate_name: String = get_arg(kwargs, "crate");
        let crate_root: String = get_arg(kwargs, "crate_root");
        let edition: String = get_arg(kwargs, "edition");
        let target_compatible_with: Set<String> = extract_set!(kwargs, "target_compatible_with");
        let compatible_with: Set<String> = extract_set!(kwargs, "compatible_with");
        let exec_compatible_with: Set<String> = extract_set!(kwargs, "exec_compatible_with");
        let env: Map<String, String> = get_arg(kwargs, "env");
        let features: Set<String> = extract_set!(kwargs, "features");
        let rustc_flags: Set<String> = extract_set!(kwargs, "rustc_flags");
        let named_deps: Map<String, String> = get_arg(kwargs, "named_deps");
        let os_named_deps: Map<String, Map<String, String>> = get_arg(kwargs, "os_named_deps");
        let os_deps: Map<String, Set<String>> = get_arg(kwargs, "os_deps");
        let visibility: Set<String> = extract_set!(kwargs, "visibility");
        let deps: Set<String> = extract_set!(kwargs, "deps");
        Ok(RustBinary {
            name,
            srcs,
            crate_name,
            crate_root,
            edition,
            target_compatible_with,
            compatible_with,
            exec_compatible_with,
            env,
            features,
            rustc_flags,
            named_deps,
            os_named_deps,
            os_deps,
            visibility,
            deps,
        })
    }

    fn patch_from(&mut self, other: &RustBinary, patch_fields: &Set<String>) {
        // Patch target_compatible_with set
        if patch_fields.contains("target_compatible_with") {
            patch_set(
                &mut self.target_compatible_with,
                &other.target_compatible_with,
            );
        }
        // Patch compatible_with set
        if patch_fields.contains("compatible_with") {
            patch_set(&mut self.compatible_with, &other.compatible_with);
        }
        // Patch exec_compatible_with set
        if patch_fields.contains("exec_compatible_with") {
            patch_set(&mut self.exec_compatible_with, &other.exec_compatible_with);
        }
        // Patch env map
        if patch_fields.contains("env") {
            patch_map(&mut self.env, &other.env);
        }
        // Patch features set
        if patch_fields.contains("features") {
            patch_set(&mut self.features, &other.features);
        }
        // Patch rustc_flags set
        if patch_fields.contains("rustc_flags") {
            patch_set(&mut self.rustc_flags, &other.rustc_flags);
        }
        // Patch visibility set
        if patch_fields.contains("visibility") {
            patch_set(&mut self.visibility, &other.visibility);
        }

        let mut dst = DepFieldsMut {
            deps: &mut self.deps,
            os_deps: &mut self.os_deps,
            named_deps: &mut self.named_deps,
            os_named_deps: &mut self.os_named_deps,
        };
        let src = DepFieldsRef {
            deps: &other.deps,
            os_deps: &other.os_deps,
            named_deps: &other.named_deps,
            os_named_deps: &other.os_named_deps,
        };
        patch_deps_fields(patch_fields, &mut dst, &src);
    }
}

impl RustTest {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let srcs: Set<String> = extract_set!(kwargs, "srcs");
        let crate_name: String = get_arg(kwargs, "crate");
        let crate_root: String = get_arg(kwargs, "crate_root");
        let edition: String = get_arg(kwargs, "edition");
        let target_compatible_with: Set<String> = extract_set!(kwargs, "target_compatible_with");
        let compatible_with: Set<String> = extract_set!(kwargs, "compatible_with");
        let exec_compatible_with: Set<String> = extract_set!(kwargs, "exec_compatible_with");
        let env: Map<String, String> = get_arg(kwargs, "env");
        let features: Set<String> = extract_set!(kwargs, "features");
        let rustc_flags: Set<String> = extract_set!(kwargs, "rustc_flags");
        let named_deps: Map<String, String> = get_arg(kwargs, "named_deps");
        let os_named_deps: Map<String, Map<String, String>> = get_arg(kwargs, "os_named_deps");
        let os_deps: Map<String, Set<String>> = get_arg(kwargs, "os_deps");
        let visibility: Set<String> = extract_set!(kwargs, "visibility");
        let deps: Set<String> = extract_set!(kwargs, "deps");
        Ok(RustTest {
            name,
            srcs,
            crate_name,
            crate_root,
            edition,
            target_compatible_with,
            compatible_with,
            exec_compatible_with,
            env,
            features,
            rustc_flags,
            named_deps,
            os_named_deps,
            os_deps,
            visibility,
            deps,
        })
    }

    fn patch_from(&mut self, other: &RustTest, patch_fields: &Set<String>) {
        // Patch target_compatible_with set
        if patch_fields.contains("target_compatible_with") {
            patch_set(
                &mut self.target_compatible_with,
                &other.target_compatible_with,
            );
        }
        // Patch compatible_with set
        if patch_fields.contains("compatible_with") {
            patch_set(&mut self.compatible_with, &other.compatible_with);
        }
        // Patch exec_compatible_with set
        if patch_fields.contains("exec_compatible_with") {
            patch_set(&mut self.exec_compatible_with, &other.exec_compatible_with);
        }
        // Patch env map
        if patch_fields.contains("env") {
            patch_map(&mut self.env, &other.env);
        }
        // Patch features set
        if patch_fields.contains("features") {
            patch_set(&mut self.features, &other.features);
        }
        // Patch rustc_flags set
        if patch_fields.contains("rustc_flags") {
            patch_set(&mut self.rustc_flags, &other.rustc_flags);
        }
        // Patch visibility set
        if patch_fields.contains("visibility") {
            patch_set(&mut self.visibility, &other.visibility);
        }

        let mut dst = DepFieldsMut {
            deps: &mut self.deps,
            os_deps: &mut self.os_deps,
            named_deps: &mut self.named_deps,
            os_named_deps: &mut self.os_named_deps,
        };
        let src = DepFieldsRef {
            deps: &other.deps,
            os_deps: &other.os_deps,
            named_deps: &other.named_deps,
            os_named_deps: &other.os_named_deps,
        };
        patch_deps_fields(patch_fields, &mut dst, &src);
    }
}

impl BuildscriptRun {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let package_name: String = get_arg(kwargs, "package_name");
        let buildscript_rule: String = get_arg(kwargs, "buildscript_rule");
        let env: Map<String, String> = get_arg(kwargs, "env");
        let env_srcs: Set<String> = extract_set!(kwargs, "env_srcs");
        let features: Set<String> = extract_set!(kwargs, "features");
        let version: String = get_arg(kwargs, "version");
        let manifest_dir: String = get_arg(kwargs, "manifest_dir");
        let visibility: Set<String> = extract_set!(kwargs, "visibility");
        Ok(BuildscriptRun {
            name,
            package_name,
            buildscript_rule,
            env,
            env_srcs,
            features,
            version,
            manifest_dir,
            visibility,
        })
    }

    fn patch_from(&mut self, other: &BuildscriptRun, patch_fields: &Set<String>) {
        // Patch env map
        if patch_fields.contains("env") {
            patch_map(&mut self.env, &other.env);
        }
        // Patch features set
        if patch_fields.contains("features") {
            patch_set(&mut self.features, &other.features);
        }
        // Patch visibility set
        if patch_fields.contains("visibility") {
            patch_set(&mut self.visibility, &other.visibility);
        }
    }
}

impl HttpArchive {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let urls_vec: Vec<String> = get_arg(kwargs, "urls");
        let urls: Set<String> = urls_vec.into_iter().collect();
        let sha256: String = get_arg(kwargs, "sha256");
        let _type: String = get_arg(kwargs, "type");
        let strip_prefix: String = get_arg(kwargs, "strip_prefix");
        let out: Option<String> = get_arg(kwargs, "out");
        Ok(HttpArchive {
            name,
            urls,
            sha256,
            _type,
            strip_prefix,
            out,
        })
    }
}

impl FileGroup {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let srcs_tuple_binding = kwargs
            .get_item("srcs")
            .expect("Expected 'srcs' argument")
            .unwrap();
        let srcs_tuple = srcs_tuple_binding.downcast::<PyTuple>().unwrap();
        let srcs = Glob::from_py_tuple(srcs_tuple)?;
        let out: Option<String> = get_arg(kwargs, "out");
        Ok(FileGroup { name, srcs, out })
    }
}

impl CargoManifest {
    fn from_py_dict(kwargs: &Bound<'_, PyDict>) -> PyResult<Self> {
        let name: String = get_arg(kwargs, "name");
        let vendor: String = get_arg(kwargs, "vendor");
        Ok(CargoManifest { name, vendor })
    }
}

pub fn parse_buck_file(file: &Utf8PathBuf) -> PyResult<Map<String, Rule>> {
    Python::attach(|py| {
        let buck = std::fs::read_to_string(file).expect("Failed to read BUCK file");
        let python_code = format!(
            r#"
call_kwargs_list = []

def buckal_call(func):
    def wrapper(*args, **kwargs):
        global call_kwargs_list
        call_kwargs_list.append((func.__name__, kwargs))
        return func(*args, **kwargs)
    return wrapper

@buckal_call
def rust_library(*args, **kwargs):
    pass

@buckal_call
def rust_binary(*args, **kwargs):
    pass

@buckal_call
def rust_test(*args, **kwargs):
    pass

@buckal_call
def buildscript_run(*args, **kwargs):
    pass

@buckal_call
def http_archive(*args, **kwargs):
    pass

@buckal_call
def filegroup(*args, **kwargs):
    pass

@buckal_call
def cargo_manifest(*args, **kwargs):
    pass

def glob(*args, **kwargs):
    return (glob.__name__, args, kwargs)

def select(arg):
    return arg

def load(*args, **kwargs):
    pass

        {}
"#,
            buck
        );

        let mut buck_rules: Map<String, Rule> = Map::new();

        let c_str = CString::new(python_code).unwrap();

        py.run(c_str.as_c_str(), None, None)?;

        let globals_binding = py.eval(c_str!("__import__('builtins').globals()"), None, None)?;
        let globals = globals_binding.downcast::<PyDict>()?;

        let kwargs_binding = globals
            .get_item("call_kwargs_list")
            .expect("call_kwargs_list not found")
            .unwrap();
        let kwargs_list = kwargs_binding.downcast::<PyList>()?;

        for tuple in kwargs_list.iter() {
            let tuple = tuple.downcast::<PyTuple>()?;
            let binding = tuple.get_item(0).unwrap();
            let func_name = binding.downcast::<PyString>()?;
            let func_name: &str = func_name.extract().unwrap();
            let binding = tuple.get_item(1).unwrap();
            let kwargs = binding.downcast::<PyDict>()?;

            match func_name {
                "rust_library" => {
                    let rule = RustLibrary::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::RustLibrary(rule));
                }
                "rust_binary" => {
                    let rule = RustBinary::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::RustBinary(rule));
                }
                "rust_test" => {
                    let rule = RustTest::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::RustTest(rule));
                }
                "buildscript_run" => {
                    let rule = BuildscriptRun::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::BuildscriptRun(rule));
                }
                "http_archive" => {
                    let rule = HttpArchive::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::HttpArchive(rule));
                }
                "filegroup" => {
                    let rule = FileGroup::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::FileGroup(rule));
                }
                "cargo_manifest" => {
                    let rule = CargoManifest::from_py_dict(kwargs)?;
                    buck_rules.insert(func_name.to_string(), Rule::CargoManifest(rule));
                }
                _ => panic!("Unknown function name: {}", func_name),
            }
        }

        Ok(buck_rules)
    })
}

pub fn patch_buck_rules(
    existing: &Map<String, Rule>,
    to_patch: &mut [Rule],
    patch_fields: &Set<String>,
) {
    for rule in to_patch.iter_mut() {
        match rule {
            Rule::RustLibrary(new_rule) => {
                if let Some(Rule::RustLibrary(existing_rule)) = existing.get("rust_library") {
                    new_rule.patch_from(existing_rule, patch_fields);
                }
            }
            Rule::RustBinary(new_rule) => {
                if let Some(Rule::RustBinary(existing_rule)) = existing.get("rust_binary") {
                    new_rule.patch_from(existing_rule, patch_fields);
                }
            }
            Rule::RustTest(new_rule) => {
                if let Some(Rule::RustTest(existing_rule)) = existing.get("rust_test") {
                    new_rule.patch_from(existing_rule, patch_fields);
                }
            }
            Rule::BuildscriptRun(new_rule) => {
                if let Some(Rule::BuildscriptRun(existing_rule)) = existing.get("buildscript_run") {
                    new_rule.patch_from(existing_rule, patch_fields);
                }
            }
            _ => {}
        }
    }
}

fn get_arg<'a, T>(kwargs: &Bound<'a, PyDict>, key: &str) -> T
where
    T: Default + FromPyObject<'a>,
{
    kwargs
        .get_item(key)
        .unwrap_or_else(|_| panic!("Expected '{}' argument", key))
        .and_then(|v| v.extract().ok())
        .unwrap_or_default()
}
