//! Dependency resolution.
//!
//! "Dependencies put in other places" needs this to be a subsystem rather than
//! a config lookup. It owns manifest and lockfile parsing per ecosystem, the
//! module graph they imply, and the classification of each module by what can
//! actually be done with it.
//!
//! # Source discovery
//!
//! A project starts as a set of source files on disk. [`discover_sources`]
//! walks a root directory and returns the candidate source files, while keeping
//! the dependency subsystem free of grammar-specific parsing. Import resolution
//! and cycle detection layer on top of this in later slices.

/// Synthetic identifier for a module in a project graph.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub String);

/// Classification of a module in a project graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    LocalPath,
    LockedRemote,
    Sdk,
    OpaqueForeign,
}

/// Extensions treated as GSET source modules during project discovery.
pub fn source_extensions() -> &'static [&'static str] {
    &[
        "py", "pyi", "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "go", "java", "rb",
        "rs", "c", "h", "cpp", "cxx", "cc", "hpp", "hxx", "hh",
    ]
}

/// Recursively discovers candidate source files beneath `root`.
pub fn discover_sources(root: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut result = Vec::new();
    discover_into(root, root, &mut result)?;
    result.sort();
    Ok(result)
}

fn discover_into(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            // Skip common build artifacts and third-party directories.
            if let Some(name) = entry.file_name().to_str() {
                if matches!(name, "target" | "node_modules" | ".git" | "dist") {
                    continue;
                }
            }
            discover_into(root, &path, out)?;
        } else if entry.file_type()?.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| source_extensions().contains(&ext.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        {
            out.push(path);
        }
    }
    Ok(())
}

/// A directed dependency graph for modules in a project.
#[derive(Clone, Debug, Default)]
pub struct ModuleGraph {
    nodes: std::collections::BTreeMap<ModuleId, std::path::PathBuf>,
    edges: std::collections::BTreeMap<ModuleId, Vec<ModuleId>>,
    reverse: std::collections::BTreeMap<ModuleId, Vec<ModuleId>>,
}

/// An error produced while building or querying a module graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// A dependency refers to a module that was never added.
    UnknownModule(ModuleId),
    /// Adding this dependency would complete a cycle.
    Cycle(Vec<ModuleId>),
}

impl ModuleGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_module(&mut self, id: ModuleId, path: std::path::PathBuf) {
        self.nodes.insert(id.clone(), path);
        self.edges.entry(id.clone()).or_default();
        self.reverse.entry(id).or_default();
    }

    pub fn module_path(&self, id: &ModuleId) -> Option<&std::path::Path> {
        self.nodes.get(id).map(|path| path.as_path())
    }

    pub fn modules(&self) -> Vec<ModuleId> {
        self.nodes.keys().cloned().collect()
    }

    pub fn add_dependency(&mut self, from: &ModuleId, to: &ModuleId) -> Result<(), GraphError> {
        if !self.nodes.contains_key(from) {
            return Err(GraphError::UnknownModule(from.clone()));
        }
        if !self.nodes.contains_key(to) {
            return Err(GraphError::UnknownModule(to.clone()));
        }
        let deps = self.edges.entry(from.clone()).or_default();
        if !deps.contains(to) {
            deps.push(to.clone());
        }
        let dependents = self.reverse.entry(to.clone()).or_default();
        if !dependents.contains(from) {
            dependents.push(from.clone());
        }
        if self.has_path(to, from) {
            return Err(GraphError::Cycle(vec![from.clone(), to.clone()]));
        }
        Ok(())
    }

    pub fn dependencies(&self, id: &ModuleId) -> &[ModuleId] {
        self.edges
            .get(id)
            .map(|deps| deps.as_slice())
            .unwrap_or(&[])
    }

    pub fn dependents(&self, id: &ModuleId) -> &[ModuleId] {
        self.reverse
            .get(id)
            .map(|deps| deps.as_slice())
            .unwrap_or(&[])
    }

    pub fn topological_order(&self) -> Result<Vec<ModuleId>, GraphError> {
        let mut state: std::collections::BTreeMap<ModuleId, u8> =
            self.nodes.keys().cloned().map(|id| (id, 0)).collect();
        let mut order = Vec::new();
        for start in self.modules() {
            if state.get(&start).copied().unwrap_or(0) == 0 {
                self.visit(&start, &mut state, &mut order)?;
            }
        }
        Ok(order)
    }

    fn has_path(&self, from: &ModuleId, to: &ModuleId) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        let mut work = vec![from.clone()];
        while let Some(node) = work.pop() {
            if node == *to {
                return true;
            }
            if !seen.insert(node.clone()) {
                continue;
            }
            work.extend(self.dependencies(&node).iter().cloned());
        }
        false
    }

    fn visit(
        &self,
        node: &ModuleId,
        state: &mut std::collections::BTreeMap<ModuleId, u8>,
        order: &mut Vec<ModuleId>,
    ) -> Result<(), GraphError> {
        match state.get(node).copied().unwrap_or(0) {
            1 => return Err(GraphError::Cycle(vec![node.clone()])),
            2 => return Ok(()),
            _ => {}
        }
        state.insert(node.clone(), 1);
        for dep in self.dependencies(node) {
            self.visit(dep, state, order)?;
        }
        state.insert(node.clone(), 2);
        order.push(node.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_supported_source_extensions() {
        let root = std::env::temp_dir().join("gset-deps-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("a.py"), "").unwrap();
        fs::write(root.join("nested").join("b.rs"), "").unwrap();
        fs::write(root.join("ignored.txt"), "").unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target").join("c.go"), "").unwrap();

        let found = discover_sources(&root).unwrap();
        assert!(found.iter().any(|p| p.ends_with("a.py")));
        assert!(found.iter().any(|p| p.ends_with("b.rs")));
        assert!(!found.iter().any(|p| p.ends_with("ignored.txt")));
        assert!(!found.iter().any(|p| p.ends_with("c.go")));
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    #[test]
    fn detects_a_cycle() {
        let mut graph = ModuleGraph::new();
        let a = ModuleId("a".to_string());
        let b = ModuleId("b".to_string());
        graph.add_module(a.clone(), std::path::PathBuf::from("a.py"));
        graph.add_module(b.clone(), std::path::PathBuf::from("b.py"));
        assert!(graph.add_dependency(&a, &b).is_ok());
        let result = graph.add_dependency(&b, &a);
        assert!(matches!(result, Err(GraphError::Cycle(_))));
    }

    #[test]
    fn topological_order_respects_dependencies() {
        let mut graph = ModuleGraph::new();
        let a = ModuleId("a".to_string());
        let b = ModuleId("b".to_string());
        graph.add_module(a.clone(), std::path::PathBuf::from("a.py"));
        graph.add_module(b.clone(), std::path::PathBuf::from("b.py"));
        graph.add_dependency(&b, &a).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order, vec![a, b]);
    }
}

/// Resolves a dotted module or package name against a project root.
///
/// The candidate paths mirror the common Python layout (`foo.py`,
/// `foo/__init__.py`, `foo/bar.py`). Other languages can layer their own
/// rules on top of this helper later.
pub fn resolve_module_name(root: &std::path::Path, module: &str) -> Option<std::path::PathBuf> {
    if module.is_empty() {
        return None;
    }
    let parts: Vec<&str> = module.split('.').collect();
    for ext in source_extensions() {
        let mut path = root.to_path_buf();
        let last = parts.len().saturating_sub(1);
        for part in &parts[..last] {
            path.push(part);
        }
        if !parts.is_empty() {
            path.push(format!("{}.{}", parts[last], ext));
        }
        if path.is_file() {
            return Some(path);
        }
        // Package directory.
        {
            let mut dir = root.to_path_buf();
            for part in &parts {
                dir.push(part);
            }
            for ext in source_extensions() {
                let candidate = dir.join(format!("__init__.{}", ext));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    // Bare package path for names like `foo.bar` when the last component is
    // itself a directory containing an index file.
    let mut dir = root.to_path_buf();
    for part in &parts {
        dir.push(part);
    }
    if dir.is_dir() {
        for ext in source_extensions() {
            let candidate = dir.join(format!("index.{}", ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolves a dotted module or package name against the first root that
/// contains it.
pub fn resolve_module_name_in_roots(
    roots: &[std::path::PathBuf],
    module: &str,
) -> Option<std::path::PathBuf> {
    roots
        .iter()
        .find_map(|root| resolve_module_name(root, module))
}

#[cfg(test)]
mod resolver_tests {
    use super::*;

    #[test]
    fn resolves_plain_and_package_modules() {
        let root = std::env::temp_dir().join("gset-resolve-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(root.join("foo.py"), "").unwrap();
        std::fs::write(root.join("pkg").join("__init__.py"), "").unwrap();
        std::fs::write(root.join("pkg").join("bar.py"), "").unwrap();

        assert_eq!(
            resolve_module_name(&root, "foo")
                .unwrap()
                .file_name()
                .unwrap()
                .to_str(),
            Some("foo.py")
        );
        assert_eq!(
            resolve_module_name(&root, "pkg")
                .unwrap()
                .file_name()
                .unwrap()
                .to_str(),
            Some("__init__.py")
        );
        assert_eq!(
            resolve_module_name(&root, "pkg.bar")
                .unwrap()
                .file_name()
                .unwrap()
                .to_str(),
            Some("bar.py")
        );
        assert!(resolve_module_name(&root, "missing").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Ecosystem of a dependency manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ecosystem {
    Pip,
    Npm,
    Cargo,
    Maven,
    Go,
}

/// A dependency manifest located in a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyManifest {
    pub ecosystem: Ecosystem,
    pub path: std::path::PathBuf,
}

const MANIFEST_FILES: &[(&str, Ecosystem, &str)] = &[
    ("requirements.txt", Ecosystem::Pip, "requirements"),
    ("pyproject.toml", Ecosystem::Pip, "pyproject"),
    ("package.json", Ecosystem::Npm, "package"),
    ("package-lock.json", Ecosystem::Npm, "lock"),
    ("Cargo.toml", Ecosystem::Cargo, "manifest"),
    ("Cargo.lock", Ecosystem::Cargo, "lock"),
    ("pom.xml", Ecosystem::Maven, "maven"),
    ("go.mod", Ecosystem::Go, "module"),
    ("go.sum", Ecosystem::Go, "sum"),
];

/// Discovers recognised dependency manifests beneath `root`.
pub fn discover_manifests(root: &std::path::Path) -> std::io::Result<Vec<DependencyManifest>> {
    let mut result = Vec::new();
    discover_manifests_into(root, root, &mut result)?;
    result.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(result)
}

fn discover_manifests_into(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<DependencyManifest>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if matches!(name, "target" | "node_modules" | ".git" | "dist" | "build") {
                    continue;
                }
            }
            discover_manifests_into(root, &path, out)?;
        } else if entry.file_type()?.is_file() {
            if let Some(name) = entry.file_name().to_str() {
                if let Some((_, ecosystem, _)) = MANIFEST_FILES
                    .iter()
                    .find(|(candidate, _, _)| *candidate == name)
                {
                    out.push(DependencyManifest {
                        ecosystem: ecosystem.clone(),
                        path,
                    });
                }
            }
        }
    }
    let _ = root;
    Ok(())
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    #[test]
    fn discovers_common_manifests() {
        let root = std::env::temp_dir().join("gset-manifest-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("requirements.txt"), "").unwrap();
        std::fs::write(root.join("sub").join("package-lock.json"), "").unwrap();

        let manifests = discover_manifests(&root).unwrap();
        assert!(manifests.iter().any(|m| m.ecosystem == Ecosystem::Pip));
        assert!(manifests.iter().any(|m| m.ecosystem == Ecosystem::Npm));
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Classifies an imported package name for an ecosystem.
///
/// Lockfile-first resolution is the long-term source of truth, but this
/// conservative classifier gives M5 a concrete and testable path today: names
/// known to be shipped with the target runtime are `Sdk`, while everything else
/// remains `OpaqueForeign` and names the target-side package manager.
pub fn classify_import(ecosystem: &Ecosystem, package: &str) -> gset_ir::ModuleSource {
    if is_sdk_package(ecosystem, package) {
        gset_ir::ModuleSource::Sdk {
            runtime: ecosystem.runtime_name().to_string(),
        }
    } else {
        gset_ir::ModuleSource::OpaqueForeign {
            artifact: ecosystem.artifact_name().to_string(),
            requirement: package.to_string(),
        }
    }
}

impl Ecosystem {
    fn runtime_name(&self) -> &'static str {
        match self {
            Self::Pip => "python",
            Self::Npm => "node",
            Self::Cargo => "rust",
            Self::Maven => "jdk",
            Self::Go => "go",
        }
    }

    fn artifact_name(&self) -> &'static str {
        match self {
            Self::Pip => "wheel",
            Self::Npm => "npm-package",
            Self::Cargo => "crate",
            Self::Maven => "jar",
            Self::Go => "go-module",
        }
    }
}

fn is_sdk_package(ecosystem: &Ecosystem, package: &str) -> bool {
    let first = package.split(['.', '/', '\\']).next().unwrap_or(package);
    match ecosystem {
        Ecosystem::Pip => matches!(
            first,
            "sys"
                | "os"
                | "math"
                | "re"
                | "json"
                | "typing"
                | "pathlib"
                | "collections"
                | "itertools"
                | "functools"
                | "dataclasses"
        ),
        Ecosystem::Npm => matches!(
            first,
            "fs" | "path"
                | "http"
                | "https"
                | "url"
                | "os"
                | "util"
                | "crypto"
                | "events"
                | "stream"
                | "node:fs"
                | "node:path"
        ),
        Ecosystem::Cargo => first == "std" || first == "core" || first == "alloc",
        Ecosystem::Maven => first == "java" || first == "javax" || first == "kotlin",
        Ecosystem::Go => matches!(
            first,
            "fmt" | "os" | "strings" | "math" | "encoding" | "time" | "io" | "errors" | "sort"
        ),
    }
}

#[cfg(test)]
mod classification_tests {
    use super::*;

    #[test]
    fn classifies_runtime_and_foreign_packages() {
        assert!(matches!(
            classify_import(&Ecosystem::Pip, "os"),
            gset_ir::ModuleSource::Sdk { .. }
        ));
        assert!(matches!(
            classify_import(&Ecosystem::Pip, "requests"),
            gset_ir::ModuleSource::OpaqueForeign { requirement, .. } if requirement == "requests"
        ));
        assert!(matches!(
            classify_import(&Ecosystem::Go, "fmt"),
            gset_ir::ModuleSource::Sdk { .. }
        ));
    }
}

/// Lists the package names directly declared by a manifest.
pub fn declared_dependencies(ecosystem: &Ecosystem, text: &str) -> Vec<String> {
    let mut names = Vec::new();
    match ecosystem {
        Ecosystem::Pip => {
            for line in text.lines() {
                let line = line.split('#').next().unwrap_or("").trim();
                if line.is_empty() || line.starts_with("--") || line.starts_with("-") {
                    continue;
                }
                let token = line.split_whitespace().next().unwrap_or("");
                let name = token
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
                    .collect::<String>();
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
        Ecosystem::Npm => {
            let mut in_dependencies = false;
            let mut depth = 0;
            for line in text.lines() {
                let trimmed = line.trim();
                if !in_dependencies {
                    if trimmed.contains("\"dependencies\"")
                        || trimmed.contains("\"devDependencies\"")
                    {
                        in_dependencies = trimmed.contains('{');
                        depth = line.chars().filter(|c| *c == '{').count()
                            - line.chars().filter(|c| *c == '}').count();
                        collect_npm_keys(trimmed, &mut names);
                    }
                    continue;
                }
                depth += line.chars().filter(|c| *c == '{').count()
                    - line.chars().filter(|c| *c == '}').count();
                collect_npm_keys(trimmed, &mut names);
                if depth <= 0 {
                    in_dependencies = false;
                }
            }
        }
        Ecosystem::Cargo => {
            if let Ok(value) = toml::from_str::<toml::Value>(text) {
                for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(table) = value.get(key).and_then(toml::Value::as_table) {
                        for name in table.keys() {
                            names.push(name.clone());
                        }
                    }
                }
            }
        }
        Ecosystem::Go => {
            let mut in_require = false;
            for raw_line in text.lines() {
                let line = raw_line.split("//").next().unwrap_or("").trim();
                if line.starts_with("require (") {
                    in_require = true;
                    continue;
                }
                if in_require && line == ")" {
                    in_require = false;
                    continue;
                }
                let require = if in_require {
                    Some(line)
                } else if line.starts_with("require ") {
                    Some(line.trim_start_matches("require ").trim())
                } else {
                    None
                };
                if let Some(require) = require {
                    if let Some(package) = require.split_whitespace().next() {
                        names.push(package.to_string());
                    }
                }
            }
        }
        Ecosystem::Maven => {
            let mut in_artifact = false;
            for line in text.lines() {
                let trimmed = line.trim();
                if in_artifact {
                    names.push(trimmed.to_string());
                    in_artifact = false;
                } else if trimmed == "<artifactId>" {
                    in_artifact = true;
                } else if let Some(rest) = trimmed.strip_prefix("<artifactId>") {
                    if let Some(name) = rest.strip_suffix("</artifactId>") {
                        names.push(name.trim().to_string());
                    }
                }
            }
        }
    }
    dedupe(&mut names);
    names.sort();
    names
}

fn collect_npm_keys(line: &str, names: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '"' {
            let start = index + 1;
            let mut end = start;
            while end < chars.len() && chars[end] != '"' {
                end += 1;
            }
            if end < chars.len() {
                let key = chars[start..end].iter().collect::<String>();
                let rest = chars[end + 1..]
                    .iter()
                    .collect::<String>()
                    .trim_start()
                    .to_string();
                if rest.starts_with(':') {
                    names.push(key);
                }
                index = end + 1;
            } else {
                index = chars.len();
            }
        } else {
            index += 1;
        }
    }
}

fn dedupe(names: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    names.retain(|name| seen.insert(name.clone()));
}

#[cfg(test)]
mod dependency_tests {
    use super::*;

    #[test]
    fn parses_common_manifest_dependencies() {
        let requirements = "requests>=2.0\nos\n# comment\n";
        assert!(
            declared_dependencies(&Ecosystem::Pip, requirements).contains(&"requests".to_string())
        );
        assert!(declared_dependencies(&Ecosystem::Pip, requirements).contains(&"os".to_string()));

        let package = r#"{ "dependencies": { "lodash": "^4", "react": "18" } }"#;
        let npm = declared_dependencies(&Ecosystem::Npm, package);
        assert!(npm.contains(&"lodash".to_string()));
        assert!(npm.contains(&"react".to_string()));

        let cargo = declared_dependencies(&Ecosystem::Cargo, "[dependencies]\nserde = \"1\"\n");
        assert!(cargo.contains(&"serde".to_string()));
    }
}
