# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic
Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Tests are now run in parallel by default unless the `--in-process` option is
  used. This can be disabled using the new `--no-parallel` option.
- Added a basic parameter fuzzing test. This test generates 50 random parameter
  value permutations. The plugin succeeds the test if it can process random
  audio buffers and note events after setting those parameters without producing
  infinite or NaN values and without crashing.

  Future versions of CLAP validator will contain more variations on this test
  and a dedicated fuzzing subcommand for longer test runs.

- Added a test that makes sure all of the plugin's symbols resolve correctly
  when the library is loaded with `RTLD_NOW`. This test is only run on Unix-like
  platforms.
- Added a test that verifies that the descriptor stored on a `clap_plugin`
  object matches the one previously obtained from the factory.
- Added a test that calls `clap_plugin_state::load()` with an empty state and
  asserts the plugin returns `false`.
- Added missing thread safety checks in the state tests.
- Added a check to ensure that plugin factories don't contain duplicate plugin
  IDs.

### Changed

- The `features-categories` test now also accepts the CLAP 1.1.7 `note-detector`
  feature as a main category feature.
- `clap-validator list tests [--json]` now includes test descriptions.
- `clap-validator validate` now indents the wrapped output slightly less to make
  the output look a bit more consistent.
- The `--only-failed` option now also shows tests that resulted in a warning in
  addition to hard failures.
- When a plugin supports text-to-value and/or value-to-text conversions for some
  but not all of its parameters, clap-validator now includes the names of the
  parameters and the failing inputs in the error message to help pinpoint the
  issue.
- The validator now asserts the plugin is in the correct state before calling
  the plugin's functions in more places to reduce the potential surface for bugs
  in the validator itself.
- All skip and error messages saying that a plugin doesn't support a certain
  extension or factory now always use the extension's or factory's actual ID.
  This is especially helpful for tests that use draft versions of extensions.

### Fixed

- Fixed a typo in the error message when a plugin descriptor's name field is a
  null pointer.

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
