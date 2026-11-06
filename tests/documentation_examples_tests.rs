// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const DOCUMENTS: [&str; 4] = [
    "README.md",
    "README.zh_CN.md",
    "doc/user_guide.md",
    "doc/user_guide.zh_CN.md",
];

static TEMP_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dependency {
    version: String,
    features: Vec<String>,
}

#[derive(Debug, Clone)]
struct CodeBlock {
    document: String,
    start_line: usize,
    end_line: usize,
    source: String,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tokio-executor-docs-{label}-{}-{timestamp}-{id}",
            std::process::id(),
        ));
        fs::create_dir_all(&path).expect("temporary directory should be created");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_document(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

/// Extracts every complete Rust fenced block and fails instead of silently
/// skipping an empty document. Partial snippets must use the explicit
/// `rust,fragment` info string; unknown Rust fence labels are errors.
fn extract_programs(document: &str, source: &str) -> Result<Vec<CodeBlock>, String> {
    let lines: Vec<&str> = document.lines().collect();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        let line = lines[cursor].trim_start();
        if !line.starts_with("```") {
            cursor += 1;
            continue;
        }
        let info = line[3..].trim();
        // Report the first content line, matching the fenced TOML extractor.
        let start = cursor + 2;
        cursor += 1;
        let mut body = Vec::new();
        let mut closed = false;
        while cursor < lines.len() {
            if lines[cursor].trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push(lines[cursor]);
            cursor += 1;
        }
        if !closed {
            return Err(format!("{source}:{start}: unterminated fenced block"));
        }
        let end = cursor + 1;
        if info == "rust" {
            let normalized = normalize_code(&body.join("\n"));
            if normalized.is_empty() {
                return Err(format!("{source}:{start}-{end}: empty Rust code block"));
            }
            blocks.push(CodeBlock {
                document: source.to_owned(),
                start_line: start,
                end_line: end,
                source: normalized,
            });
        } else if info.starts_with("rust") && info != "rust,fragment" {
            return Err(format!("{source}:{start}-{end}: unsupported Rust fence label {info:?}"));
        }
        cursor += 1;
    }
    if blocks.is_empty() {
        return Err(format!("{source}: no complete `rust` program fences found"));
    }
    Ok(blocks)
}

fn normalize_code(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Parses the deliberately small TOML dependency-table syntax used by these
/// docs: one `[dependencies]` table, bare crate keys, quoted version strings,
/// and inline tables with `version` plus a string `features` array. It rejects
/// unsupported syntax rather than treating an unparsed snippet as valid.
fn parse_install_toml(source: &str) -> Result<BTreeMap<String, Dependency>, String> {
    let mut in_dependencies = false;
    let mut saw_dependencies = false;
    let mut dependencies = BTreeMap::new();
    for (index, raw_line) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if line != "[dependencies]" || saw_dependencies {
                return Err(format!("line {line_number}: unsupported or duplicate table {line:?}"));
            }
            in_dependencies = true;
            saw_dependencies = true;
            continue;
        }
        if !in_dependencies {
            return Err(format!("line {line_number}: dependency entry outside [dependencies]"));
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {line_number}: expected `name = value`"))?;
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        {
            return Err(format!("line {line_number}: invalid bare dependency key {key:?}"));
        }
        let dependency =
            parse_dependency_value(value.trim()).map_err(|error| format!("line {line_number}: {error}"))?;
        if dependencies.insert(key.to_owned(), dependency).is_some() {
            return Err(format!("line {line_number}: duplicate dependency {key:?}"));
        }
    }
    if !saw_dependencies {
        return Err("missing [dependencies] table".to_owned());
    }
    if dependencies.is_empty() {
        return Err("[dependencies] table is empty".to_owned());
    }
    Ok(dependencies)
}

fn parse_dependency_value(value: &str) -> Result<Dependency, String> {
    if value.starts_with('"') {
        return Ok(Dependency {
            version: parse_quoted_string(value)?,
            features: Vec::new(),
        });
    }
    if !value.starts_with('{') || !value.ends_with('}') {
        return Err("expected a quoted version or inline dependency table".to_owned());
    }
    let mut version = None;
    let mut features = Vec::new();
    let mut saw_features = false;
    let entries = split_top_level(&value[1..value.len() - 1], ',')?;
    for entry in entries {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("invalid inline table entry {entry:?}"))?;
        match key.trim() {
            "version" => {
                if version.is_some() {
                    return Err("duplicate `version` field".to_owned());
                }
                version = Some(parse_quoted_string(value.trim())?);
            }
            "features" => {
                if saw_features {
                    return Err("duplicate `features` field".to_owned());
                }
                saw_features = true;
                features = parse_string_array(value.trim())?;
            }
            other => return Err(format!("unsupported inline field {other:?}")),
        }
    }
    Ok(Dependency {
        version: version.ok_or_else(|| "missing `version` field".to_owned())?,
        features,
    })
}

fn parse_quoted_string(value: &str) -> Result<String, String> {
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err("expected a double-quoted TOML string".to_owned());
    }
    let inner = &value[1..value.len() - 1];
    if inner.contains('"') || inner.contains('\\') || inner.contains('\n') {
        return Err("escapes and embedded quotes are outside the supported TOML subset".to_owned());
    }
    if inner.is_empty() {
        return Err("version string must not be empty".to_owned());
    }
    Ok(inner.to_owned())
}

fn parse_string_array(value: &str) -> Result<Vec<String>, String> {
    if !value.starts_with('[') || !value.ends_with(']') {
        return Err("`features` must be a TOML array".to_owned());
    }
    let contents = &value[1..value.len() - 1];
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_top_level(contents, ',')?
        .into_iter()
        .map(|item| parse_quoted_string(item.trim()))
        .collect()
}

fn split_top_level(source: &str, delimiter: char) -> Result<Vec<&str>, String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        match ch {
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth < 0 {
                    return Err("unbalanced inline TOML delimiters".to_owned());
                }
            }
            ch if ch == delimiter && depth == 0 => {
                parts.push(&source[start..index]);
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if quoted || escaped || depth != 0 {
        return Err("unbalanced inline TOML delimiters or quotes".to_owned());
    }
    parts.push(&source[start..]);
    Ok(parts)
}

fn extract_toml_blocks(document: &str, source: &str) -> Result<Vec<(usize, String)>, String> {
    let lines: Vec<&str> = document.lines().collect();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        let line = lines[cursor].trim_start();
        if !line.starts_with("```") {
            cursor += 1;
            continue;
        }
        let info = line[3..].trim();
        let start_line = cursor + 1;
        cursor += 1;
        let mut body = Vec::new();
        let mut closed = false;
        while cursor < lines.len() {
            if lines[cursor].trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push(lines[cursor]);
            cursor += 1;
        }
        if !closed {
            return Err(format!("{source}:{start_line}: unterminated fenced block"));
        }
        if info == "toml" {
            blocks.push((start_line, body.join("\n")));
        }
        cursor += 1;
    }
    if blocks.is_empty() {
        return Err(format!("{source}: no TOML installation block found"));
    }
    Ok(blocks)
}

fn validate_document_dependencies(
    document: &str,
    source: &str,
    package_version: &str,
) -> Result<BTreeMap<String, Dependency>, String> {
    let blocks = extract_toml_blocks(document, source)?;
    let mut expected: Option<BTreeMap<String, Dependency>> = None;
    for (line, block) in blocks {
        let parsed =
            parse_install_toml(&block).map_err(|error| format!("{source}:{line}: invalid install TOML: {error}"))?;
        let tokio_executor = parsed
            .get("qubit-tokio-executor")
            .ok_or_else(|| format!("{source}:{line}: missing qubit-tokio-executor dependency"))?;
        let version_parts: Vec<&str> = package_version.split('.').collect();
        let expected_series = format!("{}.{}", version_parts[0], version_parts[1]);
        if tokio_executor.version != expected_series {
            return Err(format!(
                "{source}:{line}: qubit-tokio-executor must use {expected_series}, found {}",
                tokio_executor.version
            ));
        }
        let qubit_executor = parsed
            .get("qubit-executor")
            .ok_or_else(|| format!("{source}:{line}: missing qubit-executor dependency"))?;
        if qubit_executor.version != "0.9" {
            return Err(format!("{source}:{line}: missing qubit-executor dependency"));
        }
        let tokio = parsed
            .get("tokio")
            .ok_or_else(|| format!("{source}:{line}: missing Tokio dependency"))?;
        if tokio.version != "1.53" {
            return Err(format!("{source}:{line}: Tokio dependency must use version 1.53"));
        }
        for feature in ["macros", "rt-multi-thread", "time"] {
            if !tokio.features.iter().any(|item| item == feature) {
                return Err(format!("{source}:{line}: Tokio dependency missing `{feature}` feature"));
            }
        }
        if let Some(previous) = &expected {
            if previous != &parsed {
                return Err(format!("{source}:{line}: install dependency snippets disagree"));
            }
        } else {
            expected = Some(parsed);
        }
    }
    expected.ok_or_else(|| format!("{source}: no parsed install dependencies"))
}

fn package_version(root: &Path) -> String {
    let manifest = read_document(root, "Cargo.toml");
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|line| line.strip_suffix('"'))
        .expect("package manifest should declare version")
        .to_owned()
}

fn manifest_dependency(name: &str, dependency: &Dependency) -> String {
    if dependency.features.is_empty() {
        format!("{name} = {:?}", dependency.version)
    } else {
        let features = dependency
            .features
            .iter()
            .map(|feature| format!("{feature:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{name} = {{ version = {:?}, features = [{features}] }}",
            dependency.version
        )
    }
}

fn unique_programs(blocks: Vec<CodeBlock>) -> Vec<CodeBlock> {
    let mut unique = BTreeMap::new();
    for block in blocks {
        unique.entry(block.source.clone()).or_insert(block);
    }
    unique.into_values().collect()
}

fn cargo_consumer_manifest(
    package: &str,
    dependencies: &BTreeMap<String, Dependency>,
    crate_root: &Path,
    programs: &[CodeBlock],
) -> String {
    let mut manifest =
        format!("[package]\nname = {package:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\n");
    for (name, dependency) in dependencies {
        manifest.push_str(&manifest_dependency(name, dependency));
        manifest.push('\n');
    }
    manifest.push_str("\n[patch.crates-io]\nqubit-tokio-executor = { path = ");
    manifest.push_str(&format!("{:?}", crate_root.to_string_lossy()));
    manifest.push_str(" }\n");
    if let Some(executor_path) = std::env::var_os("QUBIT_EXECUTOR_PATCH_PATH") {
        manifest.push_str("qubit-executor = { path = ");
        manifest.push_str(&format!("{:?}", executor_path.to_string_lossy()));
        manifest.push_str(" }\n");
    }
    for (index, _) in programs.iter().enumerate() {
        let name = format!("doc_example_{index}");
        manifest.push_str(&format!("\n[[bin]]\nname = {name:?}\npath = \"src/bin/{name}.rs\"\n"));
    }
    manifest
}

#[test]
fn test_documentation_programs_compile_and_run_in_a_standalone_consumer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let version = package_version(&root);
    let mut dependencies = None;
    let mut programs = Vec::new();
    for relative in DOCUMENTS {
        let document = read_document(&root, relative);
        let parsed_dependencies =
            validate_document_dependencies(&document, relative, &version).unwrap_or_else(|error| panic!("{error}"));
        if let Some(expected) = &dependencies {
            assert_eq!(expected, &parsed_dependencies, "{relative} install block differs");
        } else {
            dependencies = Some(parsed_dependencies);
        }
        programs.extend(extract_programs(&document, relative).unwrap_or_else(|error| panic!("{error}")));
    }
    let programs = unique_programs(programs);
    assert!(!programs.is_empty(), "documentation contains no complete Rust programs");

    let temporary = TempDir::new("consumer");
    let source_dir = temporary.0.join("src/bin");
    fs::create_dir_all(&source_dir).expect("consumer source directory should be created");
    let dependencies = dependencies.expect("all docs should declare dependencies");
    for (index, program) in programs.iter().enumerate() {
        let name = format!("doc_example_{index}");
        let path = source_dir.join(format!("{name}.rs"));
        fs::write(&path, &program.source).unwrap_or_else(|error| {
            panic!(
                "failed to write {}:{}-{}: {error}",
                program.document, program.start_line, program.end_line
            )
        });
    }
    let manifest = cargo_consumer_manifest("tokio-executor-doc-examples", &dependencies, &root, &programs);
    fs::write(temporary.0.join("Cargo.toml"), manifest).expect("consumer manifest should be written");
    let target = temporary.0.join("target");
    for (index, program) in programs.iter().enumerate() {
        let name = format!("doc_example_{index}");
        let output = Command::new("cargo")
            .arg("run")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(temporary.0.join("Cargo.toml"))
            .arg("--bin")
            .arg(&name)
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(&temporary.0)
            .output()
            .unwrap_or_else(|error| panic!("failed to run cargo for {}: {error}", program.document));
        assert!(
            output.status.success(),
            "{}:{}-{} failed to compile/run ({:?}):\nstdout:\n{}\nstderr:\n{}",
            program.document,
            program.start_line,
            program.end_line,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
}

#[test]
fn test_install_parser_rejects_invalid_toml_and_missing_required_features() {
    assert!(parse_install_toml("[dependencies]\ntokio { version = \"1\" }").is_err());
    assert!(parse_install_toml("[dependencies]\ntokio = \"1\"\ntokio = \"1\"").is_err());
    let missing_feature = "```toml\n[dependencies]\nqubit-tokio-executor = \"0.12\"\nqubit-executor = \"0.9\"\ntokio = { version = \"1.53\", features = [\"macros\", \"time\"] }\n```";
    assert!(validate_document_dependencies(missing_feature, "missing-feature.md", "0.12.0").is_err());
}

#[test]
fn test_extractor_rejects_documents_without_complete_programs() {
    assert!(extract_programs("# guide\n\n```bash\necho no\n```", "empty.md").is_err());
    assert!(extract_programs("```rust,fragment\nlet value = 42;\n```", "fragment-only.md").is_err());
    let mixed = "```rust,fragment\nlet value = 42;\n```\n```rust\nfn main() {}\n```";
    assert_eq!(extract_programs(mixed, "mixed.md").unwrap().len(), 1);
}

#[test]
fn test_extracted_compile_error_program_is_reported_with_its_source_location() {
    let source = "```rust\nfn main() { let _: u8 = \"not a number\"; }\n```";
    let program = extract_programs(source, "compile-error.md").expect("Rust block is extracted");
    let program = &program[0];
    let temporary = TempDir::new("compile-error");
    let rust_source = temporary.0.join("bad.rs");
    fs::write(&rust_source, &program.source).expect("negative example should be written");
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-name=bad_document_example")
        .arg("--emit=metadata")
        .arg(&rust_source)
        .current_dir(&temporary.0)
        .output()
        .expect("rustc should be available for the negative case");
    assert!(!output.status.success(), "compile-error block unexpectedly compiled");
    assert_eq!(program.document, "compile-error.md");
    assert_eq!(program.start_line, 2);
    assert_eq!(program.end_line, 3);
}
