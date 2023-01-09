# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic
Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2022-01-09

### Added

- There's a new command for listing the available tests available through
  `clap-validator list tests`.

### Changed

- The test verifying that the plugin can be scanned in under 100 milliseconds no
  longer emits a fatal error on failure and now emits warning instead.
- The `clap-validator list` command to print a list of installed plugins has
  been changed to `clap-validator list plugins`.

## [0.1.0] - 2022-12-12

### Added

- First tagged version after moving to the `free-audio` organization on GitHub.
