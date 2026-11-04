//! The `gset` command-line interface.
//!
//! Verb structure is preserved from the Go CLI so existing invocations and docs
//! keep working: `transpile`, `run`, `version`, `help`.
//!
//! # Startup cost
//!
//! Invocation latency is the dominant cost in a per-invocation transpiler, so
//! the binary must not touch anything expensive before it has parsed its
//! arguments. In particular no grammar may be initialised, and no language
//! registry may be populated, until `transpile` or `run` has established which
//! language is actually needed. Grammars are feature-gated for the same
//! reason: linking every grammar into every binary costs startup even if none
//! is used.
//!
//! The Go binary had no equivalent structure. It called `config.LoadConfig("")`,
//! which resolved against `os.Getwd()` and read the first `gset.conf` found on
//! disk, before it had even looked at the arguments.

use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use gset_backend::TargetId;
use gset_cli::{Output, PipelineError, transpile_with_options};

mod mcp;
mod validate;

#[derive(Parser, Debug)]
#[command(
    name = "gset",
    version,
    about = "Transpile any language to any language",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Transpile a source file and print the result to stdout
    Transpile {
        /// Source file or project directory to transpile
        path: PathBuf,
        /// Target language to emit
        #[arg(long, short = 't', visible_alias = "target", default_value = "go")]
        to: String,
        /// Additional search roots for resolving project modules
        #[arg(long = "dep-path", value_name = "PATH")]
        dep_paths: Vec<PathBuf>,
        /// Source language, overriding the one inferred from the extension
        #[arg(long, short = 'f')]
        from: Option<String>,
        /// Write the result here instead of to stdout
        #[arg(long, short = 'o')]
        output: Option<PathBuf>,
        /// Print diagnostics even when there are none
        #[arg(long)]
        verbose: bool,
        /// Enable experimental backend-specific API mappings
        #[arg(long, name = "experimental-api-map")]
        experimental_api_map: bool,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
        /// Run the native validation gate
        #[arg(long)]
        validate: bool,
    },
    /// Transpile a source file, then execute it on the target runtime
    Run {
        /// Source file to transpile and run
        path: PathBuf,
        /// Target language to emit before running
        #[arg(long, short = 't', visible_alias = "target", default_value = "go")]
        to: String,
        /// Source language, overriding the one inferred from the extension
        #[arg(long, short = 'f')]
        from: Option<String>,
        /// Keep the generated file instead of deleting it after the run
        #[arg(long)]
        keep: bool,
        /// Enable experimental backend-specific API mappings
        #[arg(long, name = "experimental-api-map")]
        experimental_api_map: bool,
    },
    /// Show backend capability matrix
    Capabilities {
        /// Target language, optional
        target: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Validate source or generated target code
    Validate {
        /// Source or project file to validate
        path: PathBuf,
        /// Target language to validate
        #[arg(long, short = 't', visible_alias = "target", default_value = "rust")]
        to: String,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Run GSET as an MCP server over stdio
    Mcp,
    /// Print the version
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Transpile {
            path,
            to,
            dep_paths,
            from,
            output,
            verbose,
            experimental_api_map,
            json,
            validate,
        } => transpile_command(
            &path,
            &to,
            from.as_deref(),
            &dep_paths,
            output.as_deref(),
            verbose,
            experimental_api_map,
            validate,
            json,
        ),
        Command::Run {
            path,
            to,
            from,
            keep,
            experimental_api_map,
        } => run_command(&path, &to, from.as_deref(), keep, experimental_api_map),
        Command::Capabilities { target, json } => capabilities_command(target.as_deref(), json),
        Command::Validate { path, to, json } => validate::validate_command(&path, &to, json),
        Command::Mcp => mcp::serve(),
        Command::Version => {
            println!("GSET v{}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn transpile_directory(
    root: &Path,
    target: &str,
    from: Option<&str>,
    dep_paths: &[PathBuf],
    destination: Option<&Path>,
    verbose: bool,
    experimental_api_map: bool,
    validate: bool,
    json: bool,
) -> ExitCode {
    if experimental_api_map {
        gset_backend::set_experimental_api_map(true);
    }
    let mut all_roots = Vec::new();
    all_roots.push(root.to_path_buf());
    all_roots.extend(dep_paths.iter().cloned());
    let sources = match gset_deps::discover_sources(root) {
        Ok(sources) => sources,
        Err(error) => {
            eprintln!("gset: cannot read {}: {error}", root.display());
            return ExitCode::from(1);
        }
    };

    if sources.is_empty() {
        eprintln!("gset: no source files found in {}", root.display());
        return ExitCode::from(2);
    }

    let mut search_sources = sources.clone();
    for dep_root in dep_paths {
        if let Ok(extra) = gset_deps::discover_sources(dep_root) {
            for path in extra {
                if !search_sources.contains(&path) {
                    search_sources.push(path);
                }
            }
        } else {
            eprintln!("gset: cannot read dependency root {}", dep_root.display());
        }
    }
    search_sources.sort();
    let source_roots: std::collections::BTreeMap<PathBuf, PathBuf> = search_sources
        .iter()
        .map(|source| {
            let owner = all_roots
                .iter()
                .rev()
                .find(|candidate| source.starts_with(candidate))
                .cloned()
                .unwrap_or_else(|| root.to_path_buf());
            (source.clone(), owner)
        })
        .collect();

    use rayon::prelude::*;

    let mut project_failed = false;
    let lowered: Vec<Result<(PathBuf, String, gset_ir::Lowered), ()>> = search_sources
        .par_iter()
        .map(|source| {
            let text = match std::fs::read_to_string(source) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("gset: cannot read {}: {error}", source.display());
                    return Err(());
                }
            };
            let frontend = match from {
                Some(language) => gset_frontend::frontend_by_name(language),
                None => gset_frontend::frontend_for_path(source),
            };
            let Some(frontend) = frontend else {
                eprintln!(
                    "gset: {}: no frontend for this source; supported files only",
                    source.display()
                );
                return Err(());
            };
            let lowered = frontend.lower(&source.to_string_lossy(), &text);
            for diagnostic in lowered.diagnostics.iter() {
                eprintln!(
                    "gset: {}: {}",
                    source.display(),
                    diagnostic.render_one_line(&lowered.source_map)
                );
                if diagnostic.severity.is_error() {
                    eprintln!("gset: {}", source.display());
                }
            }
            Ok((source.clone(), text, lowered))
        })
        .collect();

    let mut lowered_sources: Vec<(PathBuf, String, gset_ir::Lowered)> = Vec::new();
    let mut module_names: std::collections::BTreeMap<String, PathBuf> =
        std::collections::BTreeMap::new();
    for result in lowered {
        match result {
            Ok((source, text, lowered)) => {
                let owner = source_roots
                    .get(&source)
                    .cloned()
                    .unwrap_or_else(|| root.to_path_buf());
                module_names.insert(module_name_for(&owner, &source), source.clone());
                lowered_sources.push((source, text, lowered));
            }
            Err(()) => project_failed = true,
        }
    }

    let mut source_ecosystems: std::collections::BTreeMap<PathBuf, gset_deps::Ecosystem> =
        std::collections::BTreeMap::new();
    let mut source_declared_versions: std::collections::BTreeMap<
        PathBuf,
        std::collections::BTreeMap<String, Option<String>>,
    > = std::collections::BTreeMap::new();
    for source in &search_sources {
        let owner = source_roots
            .get(source)
            .cloned()
            .unwrap_or_else(|| root.to_path_buf());
        let ecosystem = ecosystem_for_source(source);
        let versions = gset_deps::manifest_dependency_versions(&owner, &ecosystem);
        source_ecosystems.insert(source.clone(), ecosystem);
        source_declared_versions.insert(source.clone(), versions);
    }

    let mut graph = gset_deps::ModuleGraph::new();
    let mut external_dependencies = std::collections::BTreeSet::new();
    for (id, path) in &module_names {
        graph.add_module(gset_deps::ModuleId(id.clone()), path.clone());
    }
    for (source, _text, lowered) in &mut lowered_sources {
        if let Some(module) = &mut lowered.module {
            let owner = source_roots
                .get(source)
                .cloned()
                .unwrap_or_else(|| root.to_path_buf());
            let from_id = gset_deps::ModuleId(module_name_for(&owner, source));
            for item in &mut module.items {
                if let gset_ir::Item::Import(import) = item {
                    let name = import
                        .path
                        .segments
                        .iter()
                        .map(|segment| segment.to_string())
                        .collect::<Vec<_>>()
                        .join(".");
                    if module_names.contains_key(&name) {
                        import.resolution = Some(gset_ir::ModuleSource::LocalPath {
                            relative: name.clone(),
                        });
                        if let Err(error) =
                            graph.add_dependency(&from_id, &gset_deps::ModuleId(name.clone()))
                        {
                            eprintln!("gset: {source:?}: import cycle: {error:?}");
                            project_failed = true;
                        }
                    } else if !name.is_empty() {
                        let ecosystem = source_ecosystems
                            .get(source)
                            .cloned()
                            .unwrap_or_else(|| ecosystem_for_source(source));
                        let version = source_declared_versions
                            .get(source)
                            .and_then(|versions| versions.get(&name))
                            .and_then(|version| version.as_deref());
                        let resolution = gset_deps::classify_import(&ecosystem, &name, version);
                        let dependency_node = gset_deps::ModuleId(format!("{ecosystem:?}::{name}"));
                        graph.add_module(dependency_node.clone(), std::path::PathBuf::from(&name));
                        if let Err(error) = graph.add_dependency(&from_id, &dependency_node) {
                            eprintln!("gset: {source:?}: dependency graph: {error:?}");
                        }
                        external_dependencies.insert(name.clone());
                        match &resolution {
                            gset_ir::ModuleSource::Sdk { runtime } => {
                                eprintln!(
                                    "gset: {}: using {runtime} runtime module `{name}`",
                                    source.display()
                                );
                            }
                            gset_ir::ModuleSource::OpaqueForeign {
                                requirement,
                                artifact,
                            } => {
                                eprintln!(
                                    "gset: {}: dependency `{requirement}` is {artifact}; \
                                     provide it with the target package manager",
                                    source.display()
                                );
                                for alternative in
                                    gset_deps::dependency_alternatives(&ecosystem, &name)
                                {
                                    eprintln!(
                                        "gset: {}: possible target-side alternative for `{requirement}`: `{alternative}`",
                                        source.display()
                                    );
                                }
                            }
                            gset_ir::ModuleSource::LockedRemote { version, .. } => {
                                eprintln!(
                                    "gset: {}: using `{version}` from project manifest",
                                    source.display()
                                );
                            }
                            _ => {}
                        }
                        import.resolution = Some(resolution);
                    }
                }
            }
        }
    }
    if let Err(error) = graph.topological_order() {
        eprintln!("gset: project contains an import cycle: {error:?}");
        project_failed = true;
    }

    // Retain each loaded module long enough to infer and emit it as one unit,
    // so import resolution is not lost between lowering and code generation.
    let mut project_files: Vec<(
        PathBuf,
        String,
        Option<gset_ir::Module>,
        gset_ir::SourceMap,
        gset_ir::DiagnosticBag,
    )> = lowered_sources
        .into_iter()
        .map(|(source, text, lowered)| {
            (
                source,
                text,
                lowered.module,
                lowered.source_map,
                lowered.diagnostics,
            )
        })
        .collect();
    let module_indices: Vec<usize> = project_files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.2.is_some())
        .map(|(index, _)| index)
        .collect();
    let mut project_modules: Vec<gset_ir::Module> = module_indices
        .iter()
        .map(|&index| project_files[index].2.take().expect("module is present"))
        .collect();
    let project_diagnostics = gset_semantic::infer_project(&mut project_modules);
    for (index, module) in module_indices.iter().zip(project_modules) {
        project_files[*index].2 = Some(module);
    }
    if !project_diagnostics.is_empty() {
        eprintln!("gset: project inference:");
        for diagnostic in project_diagnostics.iter() {
            eprintln!("  {:?}: {}", diagnostic.severity, diagnostic.message);
        }
    }

    let content_hash = project_content_hash(root, target, from, &sources);
    let output_root = destination
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("gset.out"));
    if content_hash_matches(&output_root, content_hash) {
        eprintln!("gset: project outputs up to date");
        return ExitCode::SUCCESS;
    }

    let extension = TargetId::from_name(target)
        .map(|id| id.extension())
        .unwrap_or("txt");

    let mut rust_modules = Vec::new();
    let mut overall_failed = project_failed;
    for (source, text, module, source_map, lowered_diagnostics) in &project_files {
        if sources.contains(source) {
            eprintln!("gset: {}", source.display());
        } else {
            continue;
        }

        let backend = gset_backend::backend_by_name(target);
        let (mut text_output, mut failed) = (String::new(), false);
        let mut diagnostics = lowered_diagnostics.clone();
        if let Some(backend) = backend {
            if let Some(module) = module {
                let emitted = gset_backend::emit(backend.as_ref(), module, source_map);
                if emitted.has_errors() {
                    failed = true;
                }
                diagnostics.extend(emitted.diagnostics);
                text_output = emitted.text;
            } else {
                text_output = text.clone();
                failed = true;
            }
        } else {
            eprintln!(
                "gset: {}: no backend for target `{target}`",
                source.display()
            );
            failed = true;
        }

        let output = gset_cli::Output {
            text: text_output,
            diagnostics,
            source_map: source_map.clone(),
            failed,
        };
        if output.failed {
            overall_failed = true;
        }
        report(&output, verbose);

        let relative = source.strip_prefix(root).unwrap_or(source);
        let module_name = if target == "rust" {
            relative
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| {
                    let mut out = s
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                        .collect::<String>();
                    if out == "main" {
                        out = "entry_main".to_string();
                    }
                    if out.is_empty() {
                        "module".to_string()
                    } else if !out.chars().next().unwrap().is_ascii_alphabetic()
                        && !out.starts_with('_')
                    {
                        format!("_{}", out)
                    } else {
                        out
                    }
                })
                .unwrap_or_else(|| "module".to_string())
        } else {
            relative
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module")
                .to_string()
        };
        let mut destination_path = if target == "rust" {
            output_root.join("src").join(format!("{}.rs", module_name))
        } else {
            output_root.join(relative)
        };
        destination_path.set_extension(extension);
        if let Some(parent) = destination_path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            eprintln!("gset: cannot create {}: {error}", parent.display());
            overall_failed = true;
            continue;
        }
        let output_text = if target == "rust" {
            output.text.replacen("fn main() {", "pub fn main() {", 1)
        } else {
            output.text.clone()
        };
        if let Err(error) = std::fs::write(&destination_path, &output_text) {
            eprintln!("gset: cannot write {}: {error}", destination_path.display());
            overall_failed = true;
            continue;
        }
        if target == "rust" && !overall_failed {
            rust_modules.push(module_name.clone());
        }
    }

    if overall_failed {
        ExitCode::from(1)
    } else {
        if target == "rust" {
            let cargo_manifest = output_root.join("Cargo.toml");
            let manifest = r#"[package]
name = "gset-rust-project"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "gset-rust-project"
path = "src/main.rs"

[dependencies]
"#;
            let _ = std::fs::write(&cargo_manifest, manifest);
            let src_root = output_root.join("src");
            let _ = std::fs::create_dir_all(&src_root);
            let mut main_source = String::new();
            rust_modules.sort();
            rust_modules.dedup();
            for module in &rust_modules {
                main_source.push_str(&format!(
                    "mod {};
",
                    module
                ));
            }
            if rust_modules.is_empty() {
                main_source.push_str(
                    "fn main() {}
",
                );
            } else {
                main_source.push_str(
                    "
fn main() {
",
                );
                for module in &rust_modules {
                    main_source.push_str(&format!(
                        "    {}::main();
",
                        module
                    ));
                }
                main_source.push_str(
                    "}
",
                );
            }
            let _ = std::fs::write(src_root.join("main.rs"), &main_source);
        }
        let report_path = output_root.join("gset.port-report.txt");
        let generated_files = std::fs::read_dir(&output_root)
            .into_iter()
            .flatten()
            .filter(|entry| entry.is_ok())
            .count();
        let modules = rust_modules.clone();
        let mut module_names = modules.clone();
        module_names.sort();
        module_names.dedup();
        let report_text = format!(
            "GSET port report\nsource_root: {}\ntarget: {}\ngenerated files: {}\nmodules: {} ({} files)\nmodule_names: {}\n",
            root.display(),
            target,
            generated_files,
            module_names.len(),
            rust_modules.len(),
            module_names.join(", ")
        );
        let dependency_text = external_dependencies
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ");
        let report_text = format!(
            "{}{}",
            report_text,
            if dependency_text.is_empty() {
                String::new()
            } else {
                format!("dependencies: {}\n", dependency_text)
            }
        );
        let _ = std::fs::write(&report_path, report_text);

        let mut validation_failed = false;
        if validate {
            for file in collect_generated_files(&output_root, extension.trim_start_matches('.')) {
                let report = validate::validate_file(target, &file);
                if report.status == validate::NativeValidation::Failed {
                    validation_failed = true;
                    eprintln!("gset: native validation failed for {}", file.display());
                }
            }
        }

        if json {
            let json = format!(
                "{{\"source_root\":{},\"target\":\"{}\",\"generated_files\":{},\"modules\":{},\"report\":{}}}",
                json_escape(&root.display().to_string()),
                target,
                generated_files,
                json_array(&module_names),
                json_escape(&report_path.display().to_string())
            );
            println!("{}", json);
        } else {
            eprintln!("gset: migration report: {}", report_path.display());
        }
        if json {
            eprintln!("gset: output report {}", report_path.display());
        }
        let _ = validate;
        write_content_hash(&output_root, content_hash);
        if validation_failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }
}

fn collect_generated_files(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(collect_generated_files(&path, extension));
            } else if extension.is_empty()
                || path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext == extension)
                    .unwrap_or(false)
            {
                files.push(path);
            }
        }
    }
    files
}

fn json_escape(value: &str) -> String {
    let mut result = String::from("\"");
    for ch in value.chars() {
        if ch == '"' {
            result.push_str("\\\"");
        } else if ch == '\\' {
            result.push_str("\\\\");
        } else if ch == '\n' {
            result.push_str("\\n");
        } else if ch == '\r' {
            result.push_str("\\r");
        } else if ch == '\t' {
            result.push_str("\\t");
        } else if ch.is_control() {
            result.push_str(&format!("\\u{:04x}", ch as u32));
        } else {
            result.push(ch);
        }
    }
    result.push('"');
    result
}

fn json_array(values: &[String]) -> String {
    let mut out = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&json_escape(value));
    }
    out.push(']');
    out
}

fn ecosystem_for_source(path: &Path) -> gset_deps::Ecosystem {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx") => gset_deps::Ecosystem::Npm,
        Some("rs") => gset_deps::Ecosystem::Cargo,
        Some("go") => gset_deps::Ecosystem::Go,
        Some("java") => gset_deps::Ecosystem::Maven,
        _ => gset_deps::Ecosystem::Pip,
    }
}

fn module_name_for(root: &Path, source: &Path) -> String {
    let relative = source.strip_prefix(root).unwrap_or(source);
    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        if let std::path::Component::Normal(name) = component
            && let Some(value) = name.to_str()
        {
            parts.push(value.to_string());
        }
    }
    let Some(last) = parts.pop() else {
        return String::new();
    };
    let stem = last
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(last.as_str());
    if stem == "__init__" || stem == "index" {
        return parts.join(".");
    }
    if parts.is_empty() {
        stem.to_string()
    } else {
        parts.push(stem.to_string());
        parts.join(".")
    }
}

fn project_content_hash(root: &Path, target: &str, from: Option<&str>, sources: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    target.hash(&mut hasher);
    from.hash(&mut hasher);
    for source in sources {
        source.hash(&mut hasher);
        if let Ok(text) = std::fs::read_to_string(source) {
            text.hash(&mut hasher);
        }
    }
    if let Ok(manifests) = gset_deps::discover_manifests(root) {
        for manifest in manifests {
            manifest.path.hash(&mut hasher);
            if let Ok(text) = std::fs::read_to_string(&manifest.path) {
                text.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

fn content_hash_matches(root: &Path, hash: u64) -> bool {
    let path = root.join(".gsetcache");
    std::fs::read_to_string(&path)
        .map(|value| value.trim() == hash.to_string())
        .unwrap_or(false)
}

fn write_content_hash(root: &Path, hash: u64) {
    let _ = std::fs::write(root.join(".gsetcache"), hash.to_string());
}

#[allow(clippy::too_many_arguments)]
fn transpile_command(
    path: &Path,
    target: &str,
    from: Option<&str>,
    dep_paths: &[PathBuf],
    destination: Option<&Path>,
    verbose: bool,
    experimental_api_map: bool,
    validate: bool,
    json: bool,
) -> ExitCode {
    if path.is_dir() {
        return transpile_directory(
            path,
            target,
            from,
            dep_paths,
            destination,
            verbose,
            experimental_api_map,
            validate,
            json,
        );
    }
    if experimental_api_map {
        gset_backend::set_experimental_api_map(true);
    }
    let output = match load(path, target, from, experimental_api_map) {
        Ok(output) => output,
        Err(code) => return code,
    };

    let mut report_validated = None;
    if validate {
        report_validated = Some(validate::validate_file(target, path));
    }

    report(&output, verbose);

    // Generated code is written even on failure...
    match destination {
        Some(destination) => {
            if let Err(error) = std::fs::write(destination, &output.text) {
                eprintln!("gset: cannot write {}: {error}", destination.display());
                return ExitCode::from(1);
            }
            if validate {
                report_validated = Some(validate::validate_file(target, destination));
            }
        }
        None => {
            if !json {
                print!("{}", output.text);
            }
        }
    }

    if let Some(validation) = report_validated.as_ref()
        && validation.status != validate::NativeValidation::Passed
    {
        eprintln!("gset: native validation {}", validation.status.code());
        if !validation.stderr.is_empty() {
            eprintln!("{}", validation.stderr);
        }
    }

    if json {
        println!("{}", output.to_json(&path.display().to_string(), target));
    }

    if output.failed {
        ExitCode::from(1)
    } else if let Some(validation) = report_validated {
        if validation.status == validate::NativeValidation::Failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    } else {
        ExitCode::SUCCESS
    }
}

fn run_command(
    path: &Path,
    target: &str,
    from: Option<&str>,
    keep: bool,
    experimental_api_map: bool,
) -> ExitCode {
    let output = match load(path, target, from, experimental_api_map) {
        Ok(output) => output,
        Err(code) => return code,
    };
    report(&output, false);
    if output.failed {
        // For `run` the generated source is not the product, so it goes to
        // stderr where it explains the failure without polluting stdout.
        eprint!("{}", output.text);
        return ExitCode::from(1);
    }

    let Some(id) = TargetId::from_name(target) else {
        eprintln!("gset: `{target}` is not a target this build understands");
        return ExitCode::from(2);
    };
    let directory = temp_directory(path);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!("gset: cannot create {}: {error}", directory.display());
        return ExitCode::from(1);
    }
    let program = directory.join(format!("program.{}", id.extension()));
    if let Err(error) = std::fs::write(&program, &output.text) {
        eprintln!("gset: cannot write {}: {error}", program.display());
        return ExitCode::from(1);
    }

    let code = if id == TargetId::JAVA {
        run_java(&directory, &program)
    } else if id == TargetId::RUST {
        run_rust(&directory, &program)
    } else {
        let Some(mut command) = runtime_command(id) else {
            eprintln!("gset: running {id} is not supported yet");
            if !keep {
                let _ = std::fs::remove_dir_all(&directory);
            }
            return ExitCode::from(2);
        };
        command.arg(&program);
        match command.status() {
            Ok(status) => ExitCode::from(
                status
                    .code()
                    .filter(|code| (0..=255).contains(code))
                    .unwrap_or(1) as u8,
            ),
            Err(error) => {
                eprintln!("gset: cannot run {id}: {error}");
                ExitCode::from(127)
            }
        }
    };

    if keep {
        eprintln!("gset: kept {}", program.display());
    } else {
        let _ = std::fs::remove_dir_all(&directory);
    }
    code
}

/// The runtime invocation for a target, before the generated file is appended.
fn runtime_command(target: TargetId) -> Option<ProcessCommand> {
    match target {
        TargetId::GO => {
            let mut command = ProcessCommand::new("go");
            command.arg("run");
            Some(command)
        }
        TargetId::PYTHON => Some(ProcessCommand::new("python3")),
        TargetId::JAVASCRIPT => Some(ProcessCommand::new("node")),
        TargetId::RUBY => Some(ProcessCommand::new("ruby")),
        _ => None,
    }
}

fn run_java(directory: &Path, program: &Path) -> ExitCode {
    let compile = ProcessCommand::new("javac")
        .current_dir(directory)
        .arg(program)
        .status();
    match compile {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return ExitCode::from(
                status
                    .code()
                    .filter(|code| (0..=255).contains(code))
                    .unwrap_or(1) as u8,
            );
        }
        Err(error) => {
            eprintln!("gset: cannot compile Java source: {error}");
            return ExitCode::from(127);
        }
    }

    let execution = ProcessCommand::new("java")
        .current_dir(directory)
        .arg("Program")
        .status();
    match execution {
        Ok(status) => ExitCode::from(
            status
                .code()
                .filter(|code| (0..=255).contains(code))
                .unwrap_or(1) as u8,
        ),
        Err(error) => {
            eprintln!("gset: cannot run Java program: {error}");
            ExitCode::from(127)
        }
    }
}

fn run_rust(directory: &Path, program: &Path) -> ExitCode {
    let output_path = directory.join("program");
    let compile = ProcessCommand::new("rustc")
        .current_dir(directory)
        .arg("--edition")
        .arg("2024")
        .arg(program)
        .arg("-o")
        .arg(&output_path)
        .status();
    match compile {
        Ok(status) if status.success() => {}
        Ok(status) => {
            return ExitCode::from(
                status
                    .code()
                    .filter(|code| (0..=255).contains(code))
                    .unwrap_or(1) as u8,
            );
        }
        Err(error) => {
            eprintln!("gset: cannot compile Rust source: {error}");
            return ExitCode::from(127);
        }
    }

    let execution = ProcessCommand::new(&output_path).status();
    match execution {
        Ok(status) => ExitCode::from(
            status
                .code()
                .filter(|code| (0..=255).contains(code))
                .unwrap_or(1) as u8,
        ),
        Err(error) => {
            eprintln!("gset: cannot run Rust program: {error}");
            ExitCode::from(127)
        }
    }
}

/// A per-invocation directory under the system temp directory.
///
/// The name includes the process id and a nanosecond timestamp so two `gset`
/// processes never collide, and includes the source stem so a `--keep` run
/// leaves a recognisable directory.
fn temp_directory(source: &Path) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("gset");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("gset-{stem}-{}-{nanos}", std::process::id()))
}

fn load(
    path: &Path,
    target: &str,
    from: Option<&str>,
    experimental_api_map: bool,
) -> Result<Output, ExitCode> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("gset: cannot read {}: {error}", path.display());
            return Err(ExitCode::from(1));
        }
    };
    let name = path.to_string_lossy();

    gset_backend::set_experimental_api_map(experimental_api_map);
    match transpile_with_options(&name, &text, from, target, experimental_api_map) {
        Ok(output) => Ok(output),
        Err(error) => {
            print_pipeline_error(&error);
            Err(ExitCode::from(2))
        }
    }
}

fn print_pipeline_error(error: &PipelineError) {
    eprintln!("gset: {error}");
}

fn report(output: &Output, verbose: bool) {
    for diagnostic in output.diagnostics.iter() {
        eprintln!("{}", diagnostic.render_one_line(&output.source_map));
    }
    if verbose && output.diagnostics.is_empty() {
        eprintln!("gset: no diagnostics");
    }
}

fn capabilities_command(target: Option<&str>, json: bool) -> ExitCode {
    let target = target.unwrap_or("rust");
    let Some(backend) = gset_backend::backend_by_name(target) else {
        eprintln!("gset: no backend for `{target}`");
        return ExitCode::from(2);
    };
    let mut capabilities = Vec::new();
    for capability in CapabilityList::all() {
        capabilities.push((capability.name(), backend.capability(capability).code()));
    }
    if json {
        let mut out = String::from("[");
        for (index, (capability, support)) in capabilities.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"capability\":\"{}\",\"support\":\"{}\"}}",
                capability, support
            ));
        }
        out.push(']');
        println!("{}", out);
    } else {
        for (capability, support) in capabilities {
            println!("{capability:28} {support}");
        }
    }
    ExitCode::SUCCESS
}

struct CapabilityList;
impl CapabilityList {
    fn all() -> Vec<gset_backend::Capability> {
        vec![
            gset_backend::Capability::Imports,
            gset_backend::Capability::Classes,
            gset_backend::Capability::Records,
            gset_backend::Capability::Enums,
            gset_backend::Capability::Interfaces,
            gset_backend::Capability::TypeAliases,
            gset_backend::Capability::Comprehensions,
            gset_backend::Capability::Generators,
            gset_backend::Capability::Exceptions,
            gset_backend::Capability::ContextManagers,
            gset_backend::Capability::Decorators,
            gset_backend::Capability::Async,
            gset_backend::Capability::FStrings,
            gset_backend::Capability::Closures,
            gset_backend::Capability::PatternMatching,
            gset_backend::Capability::Slices,
            gset_backend::Capability::KeywordArguments,
            gset_backend::Capability::MultipleAssignment,
            gset_backend::Capability::OperatorCall,
            gset_backend::Capability::LoopElse,
        ]
    }
}
