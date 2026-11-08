// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::fs;

/// Reads a project file as UTF-8 text.
///
/// # Parameters
///
/// * `path` - Path relative to the crate root.
///
/// # Returns
///
/// The file contents.
fn read_project_file(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|err| panic!("failed to read {path}: {err}"))
}

/// Extracts the crate version from `Cargo.toml`.
///
/// # Returns
///
/// The package version declared by this crate.
fn package_version() -> String {
    read_project_file("Cargo.toml")
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|value| value.strip_suffix('"'))
        .expect("Cargo.toml should declare package version")
        .to_owned()
}

/// Ensures all user-facing installation snippets use the crate package
/// major.minor line.
#[test]
fn test_user_docs_use_current_crate_version() {
    let version = package_version();
    let minor_version = minor_series(&version).expect("package version should have major.minor");
    let dependency = format!("qubit-tokio-executor = \"{minor_version}\"");
    let readme_en = read_project_file("README.md");
    let readme_zh = read_project_file("README.zh_CN.md");
    let guide_en = read_project_file("doc/user_guide.md");
    let guide_zh = read_project_file("doc/user_guide.zh_CN.md");

    for (name, document) in [
        ("README.md", &readme_en),
        ("README.zh_CN.md", &readme_zh),
        ("doc/user_guide.md", &guide_en),
        ("doc/user_guide.zh_CN.md", &guide_zh),
    ] {
        assert!(document.contains(&dependency), "{name} must declare {dependency}");
    }
}

/// Returns `major.minor` from a semver string.
///
/// # Parameters
///
/// * `version` - Package version from `Cargo.toml`.
///
/// # Returns
///
/// The two-component version requirement used in README dependency examples.
fn minor_series(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    Some(format!("{major}.{minor}"))
}

/// Ensures user docs no longer document removed or renamed APIs.
#[test]
fn test_user_docs_do_not_reference_removed_api_names() {
    let readme_en = read_project_file("README.md");
    let readme_zh = read_project_file("README.zh_CN.md");
    let guide_en = read_project_file("doc/user_guide.md");
    let guide_zh = read_project_file("doc/user_guide.zh_CN.md");

    for (name, document) in [
        ("README.md", &readme_en),
        ("README.zh_CN.md", &readme_zh),
        ("doc/user_guide.md", &guide_en),
        ("doc/user_guide.zh_CN.md", &guide_zh),
    ] {
        assert!(!document.contains("RejectedExecution"), "{name} references removed API");
        assert!(!document.contains("TokioExecution"), "{name} references removed API");
    }
}

/// Ensures the published user guides' bilingual concurrency-design links point
/// to files included in the crate package.
#[test]
fn test_concurrency_design_links_resolve_to_project_files() {
    for (guide, target) in [
        ("doc/user_guide.md", "doc/concurrency-design.md"),
        ("doc/user_guide.zh_CN.md", "doc/concurrency-design.zh_CN.md"),
    ] {
        let document = read_project_file(guide);
        let link_target = target.strip_prefix("doc/").expect("target should be under doc");
        assert!(
            document.contains(&format!("]({link_target})")),
            "{guide} must link to {target}"
        );
        assert!(std::path::Path::new(target).is_file(), "{target} must exist");
    }
}
