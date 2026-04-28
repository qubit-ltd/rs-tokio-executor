# Code Formatting Guide

## Overview

This project uses **nightly Rust's `rustfmt`** for code formatting to support advanced formatting features, specifically the `imports_layout = "Vertical"` option.

## Why Nightly?

The `imports_layout` configuration option is an **unstable feature** that requires nightly Rust. This allows us to format imports with each item on its own line:

```rust
pub use module::{
    Item1,
    Item2,
    Item3,
};
```

**Official Documentation:**
- [Rustfmt Configuration Documentation](https://rust.googlesource.com/rustfmt/+/105cbf8ed517d9e42ae1dc837bdc97cc3ff28175/Configurations.md)
- [GitHub Tracking Issue #5083](https://github.com/rust-lang/rustfmt/issues/5083)

## How to Format Code

### Option 1: Using the Format Script (Recommended)

```bash
./format.sh
```

This script will:
1. Check if nightly toolchain is installed
2. Install it automatically if needed
3. Run `cargo +nightly fmt`

### Option 2: Manual Command

```bash
# Install nightly toolchain (if not already installed)
rustup toolchain install nightly

# Format code
cargo +nightly fmt
```

## CI/CD Integration

Both the local CI check script (`ci-check.sh`) and CircleCI configuration (`.circleci/config.yml`) have been configured to use nightly Rust:

- **Local checks**: Run `./ci-check.sh` before committing (automatically installs nightly if needed)
- **CircleCI**: Uses `rustlang/rust:nightly` Docker image for all jobs

## Configuration

The formatting configuration is defined in `rustfmt.toml`:

```toml
# Format imports with vertical layout (each item on its own line within braces)
imports_layout = "Vertical"
```

This is the **only configuration option** needed to achieve our desired formatting style.

## Important Notes

1. **All CI jobs use nightly Rust**: The entire CI pipeline uses nightly Rust toolchain for consistency
2. **Local development**: Can use either stable or nightly; formatting requires nightly
3. **Automatic installation**: The `ci-check.sh` script automatically installs nightly toolchain if needed
4. **Docker image**: CircleCI uses the official `rustlang/rust:nightly` Docker image
5. **No manual intervention**: Developers don't need to manually switch toolchains

## Troubleshooting

### Format check fails in CI

Make sure your code is formatted before committing:

```bash
./format.sh
```

### Nightly toolchain issues

Reinstall the nightly toolchain:

```bash
rustup toolchain uninstall nightly
rustup toolchain install nightly --component rustfmt
```

## References

- [Rustfmt Documentation](https://rust-lang.github.io/rustfmt/)
- [Rustup Documentation](https://rust-lang.github.io/rustup/)

