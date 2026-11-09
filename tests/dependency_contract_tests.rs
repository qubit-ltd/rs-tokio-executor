// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================

/// Ensures the executor dependency requirement excludes its incompatible 0.9.0
/// release.
#[test]
fn test_executor_requirement_excludes_unsupported_release() {
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let requirement = manifest["dependencies"]["qubit-executor"]["version"].as_str().unwrap();
    let requirement = semver::VersionReq::parse(requirement).unwrap();

    assert!(!requirement.matches(&semver::Version::new(0, 9, 0)));
    assert!(requirement.matches(&semver::Version::new(0, 9, 1)));
}
