# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic
Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- There's a new command for listing the available tests available through
  `clap-validator list tests`.

### Changed

- The test verifying that the plugin can be scanned in under 100 milliseconds
  now emits a non-fatal warning on failure rather than an error.
- The `clap-validator list` command to print a list of installed plugins is now
  `clap-validator list plugins`.

## [0.1.0] - 2022-12-12

### Added

- First tagged version after moving to the `free-audio` organization on GitHub.
