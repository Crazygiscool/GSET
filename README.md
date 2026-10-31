# GSET - Generic Syntax Extension Tool

<div align="center">

**Write in any language syntax, compile to any language.**

[![License: Apache-2.0 OR MIT](https://img.shields.io/badge/License-Apache--2.0%20OR%20MIT-blue)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.90+-orange?style=flat&logo=rust)](https://rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/Crazygiscool/GSETLang)](https://github.com/Crazygiscool/GSETLang/releases)
[![Tests](https://img.shields.io/github/actions/workflow/status/Crazygiscool/GSETLang/test.yml)](https://github.com/Crazygiscool/GSETLang/actions)

</div>

## Install

- **Linux / macOS**: `curl -fsSL https://raw.githubusercontent.com/Crazygiscool/GSETLang/main/install.sh | bash`
- **Windows**: `powershell -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/Crazygiscool/GSETLang/main/install.ps1 | iex"`
- **Homebrew**: `brew install crazygiscool/gset/gset`
- **AUR**: `gset` (stable); `gset-git` is deprecated in favor of it
- **Chocolatey**: `choco install gset`

- **Windows (installer)**: download `gset-<version>-windows-amd64-setup.exe`
  (choose "Install for all users" or "Install just for me") or
  `gset-<version>-windows-amd64.msi` (all users) from the release assets below.

## Publishing

GSET ships to four channels: GitHub Release, the Homebrew tap, AUR, and
Chocolatey. winget is intentionally not published (no manifests are kept in
this repository).
1. **Cut the tag** — releases build from `v*` tags:

   ```bash
   git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
   ```

   `.github/workflows/release.yml` builds the asset matrix and creates the GitHub
   Release.
2. **Publish packages** — the publish workflow does not auto-trigger for
   API-created releases, so dispatch it after the release is up:

   ```bash
   gh workflow run publish-packages.yml -R Crazygiscool/GSETLang -f tag=vX.Y.Z
   ```

   `.github/workflows/publish-packages.yml` then updates the
   [Homebrew tap](https://github.com/Crazygiscool/homebrew-gset), pushes the AUR
   `gset` package (and deprecates `gset-git`), and pushes the Chocolatey package.

   Every package step is idempotent, so re-running after a partial failure is
   safe.
