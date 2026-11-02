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
    pub rank: usize,
}

const MANIFEST_FILES: &[(&str, Ecosystem, usize)] = &[
    ("requirements.txt", Ecosystem::Pip, 0),
    ("pyproject.toml", Ecosystem::Pip, 1),
    ("package-lock.json", Ecosystem::Npm, 0),
    ("package.json", Ecosystem::Npm, 1),
    ("Cargo.lock", Ecosystem::Cargo, 0),
    ("Cargo.toml", Ecosystem::Cargo, 1),
    ("pom.xml", Ecosystem::Maven, 0),
    ("go.sum", Ecosystem::Go, 0),
    ("go.mod", Ecosystem::Go, 1),
];

/// Discovers recognised dependency manifests beneath `root`.
pub fn discover_manifests(root: &std::path::Path) -> std::io::Result<Vec<DependencyManifest>> {
    let mut result = Vec::new();
    discover_manifests_into(root, root, &mut result)?;
    result.sort_by(|left, right| {
        left.rank
            .cmp(&right.rank)
            .then_with(|| left.path.cmp(&right.path))
    });
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
                if let Some((_, ecosystem, rank)) = MANIFEST_FILES
                    .iter()
                    .find(|(candidate, _, _)| *candidate == name)
                {
                    out.push(DependencyManifest {
                        ecosystem: ecosystem.clone(),
                        path,
                        rank: *rank,
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
pub fn classify_import(
    ecosystem: &Ecosystem,
    package: &str,
    version: Option<&str>,
) -> gset_ir::ModuleSource {
    if is_sdk_package(ecosystem, package) {
        return gset_ir::ModuleSource::Sdk {
            runtime: ecosystem.runtime_name().to_string(),
        };
    }
    if let Some(version) = version {
        return gset_ir::ModuleSource::LockedRemote {
            version: version.to_string(),
            source_available: false,
        };
    }
    gset_ir::ModuleSource::OpaqueForeign {
        artifact: ecosystem.artifact_name().to_string(),
        requirement: package.to_string(),
    }
}

/// Returns declared dependencies with their version strings when available.
pub fn declared_dependency_versions(
    ecosystem: &Ecosystem,
    text: &str,
) -> Vec<(String, Option<String>)> {
    let mut result = Vec::new();
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
                let version = token[name.len()..].trim();
                if !name.is_empty() {
                    result.push((
                        name,
                        if version.is_empty() {
                            None
                        } else {
                            Some(version.to_string())
                        },
                    ));
                }
            }
        }
        Ecosystem::Npm => {
            if let Some(dependencies) = parse_npm_dependencies(text) {
                result.extend(dependencies);
            } else {
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
                            collect_npm_versions(trimmed, &mut result);
                        }
                        continue;
                    }
                    depth += line.chars().filter(|c| *c == '{').count()
                        - line.chars().filter(|c| *c == '}').count();
                    collect_npm_versions(trimmed, &mut result);
                    if depth <= 0 {
                        in_dependencies = false;
                    }
                }
            }
        }
        Ecosystem::Cargo => {
            if let Ok(value) = toml::from_str::<toml::Value>(text) {
                let mut found = false;
                for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(table) = value.get(key).and_then(toml::Value::as_table) {
                        found = true;
                        for (name, version) in table {
                            let version = if let toml::Value::String(value) = version {
                                Some(value.clone())
                            } else if let Some(table) = version.as_table() {
                                table
                                    .get("version")
                                    .and_then(toml::Value::as_str)
                                    .map(str::to_string)
                            } else {
                                None
                            };
                            result.push((name.clone(), version));
                        }
                    }
                }
                if !found {
                    if let Some(packages) = value.get("package").and_then(toml::Value::as_array) {
                        for package in packages {
                            if let Some(table) = package.as_table() {
                                let name = table.get("name").and_then(toml::Value::as_str);
                                let version = table.get("version").and_then(toml::Value::as_str);
                                if let Some(name) = name {
                                    result.push((name.to_string(), version.map(str::to_string)));
                                }
                            }
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
                    let mut parts = require.split_whitespace();
                    if let (Some(package), version) = (parts.next(), parts.next()) {
                        result.push((package.to_string(), version.map(str::to_string)));
                    }
                }
            }
        }
        Ecosystem::Maven => {
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("<artifactId>") {
                    if let Some(name) = rest.strip_suffix("</artifactId>") {
                        result.push((name.trim().to_string(), None));
                    }
                }
            }
        }
    }
    dedupe_versions(&mut result);
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}

pub fn manifest_dependency_versions(
    root: &std::path::Path,
    ecosystem: &Ecosystem,
) -> std::collections::BTreeMap<String, Option<String>> {
    let mut versions = std::collections::BTreeMap::new();
    if let Ok(manifests) = discover_manifests(root) {
        for manifest in manifests
            .into_iter()
            .filter(|manifest| manifest.ecosystem == *ecosystem)
        {
            if let Ok(text) = std::fs::read_to_string(&manifest.path) {
                for (name, version) in declared_dependency_versions(ecosystem, &text) {
                    versions.entry(name).or_insert(version);
                }
            }
        }
    }
    versions
}

fn collect_npm_versions(line: &str, names: &mut Vec<(String, Option<String>)>) {
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
                let rest = chars[end + 1..].iter().collect::<String>();
                let rest = rest.trim_start();
                if let Some(version) = rest.strip_prefix(':').map(str::trim) {
                    let version = version
                        .trim_start_matches('"')
                        .split('"')
                        .next()
                        .map(trim_version)
                        .flatten();
                    names.push((key, version));
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

fn trim_version(version: &str) -> Option<String> {
    let value = version.trim().trim_matches('"').trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_npm_dependencies(text: &str) -> Option<Vec<(String, Option<String>)>> {
    let needle = r#""dependencies""#;
    let needle_index = text.find(needle)?;
    let brace_offset = text[needle_index + needle.len()..].find('{')? + needle_index + needle.len();
    let chars: Vec<char> = text.chars().collect();
    let open = text[..brace_offset + 1].chars().count() - 1;
    let mut index = open + 1;
    let mut depth = 1;
    let mut result = Vec::new();
    skip_whitespace_chars(&chars, &mut index);
    while index < chars.len() && depth > 0 {
        if chars[index] == '}' {
            depth -= 1;
            index += 1;
            continue;
        }
        if chars[index] == ',' {
            index += 1;
            skip_whitespace_chars(&chars, &mut index);
            continue;
        }
        let key = match parse_json_string_chars(&chars, &mut index) {
            Some(key) => key,
            None => return None,
        };
        skip_whitespace_chars(&chars, &mut index);
        if index >= chars.len() || chars[index] != ':' {
            return None;
        }
        index += 1;
        skip_whitespace_chars(&chars, &mut index);
        let version = if index < chars.len() && chars[index] == '{' {
            let nested = parse_matching_object_chars(&chars, &mut index)?;
            version_from_object(&nested)
        } else if index < chars.len() && chars[index] as u32 == 34 {
            parse_json_string_chars(&chars, &mut index)
        } else {
            None
        };
        result.push((key, version));
        skip_whitespace_chars(&chars, &mut index);
    }
    eprintln!("npm result={:?}", result);
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn version_from_object(object: &str) -> Option<String> {
    let chars: Vec<char> = object.chars().collect();
    let mut index = 0;
    skip_whitespace_chars(&chars, &mut index);
    if index >= chars.len() || chars[index] != '{' {
        return None;
    }
    index += 1;
    while index < chars.len() {
        skip_whitespace_chars(&chars, &mut index);
        if index >= chars.len() || chars[index] == '}' {
            break;
        }
        if chars[index] == ',' {
            index += 1;
            continue;
        }
        let key = match parse_json_string_chars(&chars, &mut index) {
            Some(key) => key,
            None => return None,
        };
        skip_whitespace_chars(&chars, &mut index);
        if index >= chars.len() || chars[index] != ':' {
            return None;
        }
        index += 1;
        skip_whitespace_chars(&chars, &mut index);
        if key == "version" && index < chars.len() && chars[index] as u32 == 34 {
            return parse_json_string_chars(&chars, &mut index);
        }
        if index < chars.len() && chars[index] == '{' {
            let _ = parse_matching_object_chars(&chars, &mut index);
        } else if index < chars.len() && chars[index] as u32 == 34 {
            let _ = parse_json_string_chars(&chars, &mut index);
        } else {
            while index < chars.len() && chars[index] != ',' && chars[index] != '}' {
                index += 1;
            }
        }
    }
    None
}

fn parse_matching_object_chars(chars: &[char], index: &mut usize) -> Option<String> {
    if *index >= chars.len() || chars[*index] != '{' {
        return None;
    }
    let start = *index;
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    while *index < chars.len() {
        let ch = chars[*index];
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch as u32 == 34 {
                in_string = false;
            }
        } else if ch as u32 == 34 {
            in_string = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                *index += 1;
                return Some(chars[start..*index].iter().collect());
            }
        }
        *index += 1;
    }
    None
}

fn parse_json_string_chars(chars: &[char], index: &mut usize) -> Option<String> {
    if *index >= chars.len() || chars[*index] != '"' {
        return None;
    }
    *index += 1;
    let mut value = String::new();
    while *index < chars.len() {
        if chars[*index] == '"' {
            *index += 1;
            return Some(value);
        }
        value.push(chars[*index]);
        *index += 1;
    }
    None
}

fn skip_whitespace_chars(chars: &[char], index: &mut usize) {
    while *index < chars.len() && chars[*index].is_whitespace() {
        *index += 1;
    }
}

fn dedupe_versions(names: &mut Vec<(String, Option<String>)>) {
    let mut seen = std::collections::BTreeSet::new();
    names.retain(|(name, _)| seen.insert(name.clone()));
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
            classify_import(&Ecosystem::Pip, "os", None),
            gset_ir::ModuleSource::Sdk { .. }
        ));
        assert!(matches!(
            classify_import(&Ecosystem::Pip, "requests", None),
            gset_ir::ModuleSource::OpaqueForeign { requirement, .. } if requirement == "requests"
        ));
        assert!(matches!(
            classify_import(&Ecosystem::Go, "fmt", None),
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

#[cfg(test)]
mod manifest_dependency_tests {
    use super::*;

    #[test]
    fn parses_versions_from_manifests() {
        let requirements = "requests==2.0\nos\n";
        let deps = declared_dependency_versions(&Ecosystem::Pip, requirements);
        assert!(
            deps.iter()
                .any(|(name, version)| name == "requests" && version.as_deref() == Some("==2.0"))
        );

        let package = r#"{ "dependencies": { "lodash": "^4" } }"#;
        let npm = declared_dependency_versions(&Ecosystem::Npm, package);
        assert!(
            npm.iter()
                .any(|(name, version)| name == "lodash" && version.as_deref() == Some("^4"))
        );

        let package_lock = r#"{ "dependencies": { "lodash": { "version": "4.17.21" } } }"#;
        let lock_versions = declared_dependency_versions(&Ecosystem::Npm, package_lock);
        assert!(
            lock_versions
                .iter()
                .any(|(name, version)| name == "lodash" && version.as_deref() == Some("4.17.21"))
        );

        let cargo =
            declared_dependency_versions(&Ecosystem::Cargo, "[dependencies]\nserde = \"1\"\n");
        assert!(
            cargo
                .iter()
                .any(|(name, version)| name == "serde" && version.as_deref() == Some("1"))
        );

        let locked = classify_import(&Ecosystem::Pip, "requests", Some("==2.0"));
        assert!(matches!(locked, gset_ir::ModuleSource::LockedRemote { .. }));
    }
}

#[cfg(test)]
mod lockfile_priority_tests {
    use super::*;

    #[test]
    fn lockfile_version_wins_over_build_manifest() {
        let root = std::env::temp_dir().join("gset-lock-priority-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), "[dependencies]\nserde = \"2\"\n").unwrap();
        std::fs::write(
            root.join("Cargo.lock"),
            "[[package]]\nname = \"serde\"\nversion = \"1.0.200\"\n",
        )
        .unwrap();

        let versions = manifest_dependency_versions(&root, &Ecosystem::Cargo);
        assert_eq!(versions.get("serde"), Some(&Some("1.0.200".to_string())));
        let _ = std::fs::remove_dir_all(&root);
    }
}
