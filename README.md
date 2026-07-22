# clap-validator

[![Automated builds](https://github.com/blepfx/clap-validator/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/blepfx/clap-validator/actions/workflows/build.yml?query=branch%3Amaster)

A validator and automatic test suite for [CLAP](https://github.com/free-audio/clap) plugins. Clap-validator can automatically test one or more plugins for common bugs and incorrect behavior. See [CHANGELOG](./CHANGELOG.md) for a detailed list of changes and additions in each version.

## Download

Development builds can be found
[here](https://nightly.link/blepfx/clap-validator/workflows/build/master).
The macOS builds are unsigned and may require Gatekeeper to be disabled or the
quarantine bit to be removed
([instructions](https://disable-gatekeeper.github.io/)).

## Usage

Simply pass the path to one or more `.clap` plugins to `clap-validator validate`
to run the validator on those plugins. The `--only-failed` option can be used to
hide the output from all successful and skipped tests. Running `clap-validator
validate --help` lists all available options:

```shell
clap-validator validate /path/to/the/plugin.clap
clap-validator validate /path/to/the/plugin.clap --only-failed
clap-validator validate --help
```

### Filtering

By default, all tests are run during validation, including pedantic ones. You can use the `--include` option to specify a regex of tests to run, and `--exclude` to specify a regex of tests to skip. Another option is to create a configuration file named `clap-validator.toml` in the current working directory or any of its parent directories. In this file, you can specify which tests to enable or disable. An example configuration file looks like this:

```toml
# clap-validator.toml
[test]
state-reproducibility-binary = false
```

### Fuzzing

> [!WARNING] 
> Fuzzing is experimental and can contain bugs that can cause false positives even if your plugin is perfectly fine.

clap-validator comes with a built-in multi-process fuzzer that can run the plugin through a series of random parameter changes, note on/off events, and transport changes while checking for crashes, hangs, and spec-compliance issues. Use `clap-validator fuzz` to run the fuzzer (4 parallel runners for 10.5 minutes):

```shell
clap-validator fuzz -j4 -d10m30s /path/to/the/plugin.clap
```

If the fuzzer finds a crash, it will print a crash info and a seed that can be used to reproduce the issue. To reproduce a crash, use the `--reproduce` option:

```shell
clap-validator fuzz --reproduce <seed> /path/to/the/plugin.clap
```

This will run a single fuzzer "chunk" with that seed in-process, repeating the same sequence of operations that led to the issue. You can attach a debugger, or enable tracing while using `--reproduce`.

## Debugging

clap-validator runs tests in separate processes by default so plugin crashes can
be treated as such instead of taking down the validator. If you want to attach a
debugger to debug the plugin's behavior during a specific test, you can tell the
validator to run the that test in the current process. Use `clap-validator list tests`
to list all available tests.

```shell
clap-validator validate --in-process --include <test-case-name> /path/to/the/plugin.clap
```

### Tracing

> [!WARNING] 
> Tracing can cause some overhead AND because it emits events without any buffering (to avoid losing events in case of a crash), it can mask a plugin crash or change the timing of events enough to make some issues not reproducible. Use with caution.

clap-validator can generate traces of plugin/host call execution during the in-process tests that could be used to diagnose issues or understand plugin behavior. To enable tracing, pass the `--trace` option to `clap-validator validate`. The generated trace files can be opened in [Perfetto](https://perfetto.dev/).

## Building

After installing [Rust](https://rustup.rs/), you can compile and run clap-validator as follows:

```shell
cargo build --release # build the binary
./target/release/clap-validator validate /path/to/the/plugin.clap # and run it
```

or

```shell
cargo run --release -- validate /path/to/the/plugin.clap # build & run
```

