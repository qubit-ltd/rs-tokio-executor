/*******************************************************************************
 *
 *    Copyright (c) 2025 - 2026 Haixing Hu.
 *
 *    SPDX-License-Identifier: Apache-2.0
 *
 *    Licensed under the Apache License, Version 2.0.
 *
 ******************************************************************************/
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

/// Ensures README dependency snippets match the crate package version.
#[test]
fn test_readmes_use_current_crate_version() {
    let version = package_version();
    let dependency = format!("qubit-tokio-executor = \"{version}\"");
    let readme_en = read_project_file("README.md");
    let readme_zh = read_project_file("README.zh_CN.md");

    assert!(readme_en.contains(&dependency));
    assert!(readme_zh.contains(&dependency));
}

/// Ensures READMEs no longer document removed or renamed APIs.
#[test]
fn test_readmes_do_not_reference_removed_api_names() {
    let readme_en = read_project_file("README.md");
    let readme_zh = read_project_file("README.zh_CN.md");

    for readme in [&readme_en, &readme_zh] {
        assert!(!readme.contains("RejectedExecution"));
        assert!(!readme.contains("TokioExecution"));
    }
}

/// Ensures only the blocking service documents service-level async termination.
#[test]
fn test_readmes_limit_await_termination_to_blocking_service() {
    let readme_en = read_project_file("README.md");
    let readme_zh = read_project_file("README.zh_CN.md");

    for readme in [&readme_en, &readme_zh] {
        assert_eq!(
            1,
            readme.matches("service.await_termination().await").count()
        );
    }
}
