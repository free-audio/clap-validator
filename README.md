# clap-validator

[![Automated builds](https://github.com/free-audio/clap-validator/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/free-audio/clap-validator/actions/workflows/build.yml?query=branch%3Amaster)

A validator and automatic test suite for [CLAP](https://github.com/free-audio/clap) plugins. Clap-validator can automatically test one or more plugins for common bugs and incorrect behavior.

## Download

Prebuilt binaries can be found on the [releases
page](https://github.com/free-audio/clap-validator/releases). Development builds
can be found
[here](https://nightly.link/free-audio/clap-validator/workflows/build/master).
The macOS builds are unsigned and may require Gatekeeper to be disabled or the
quarantine bit to be removed
([instructions](https://disable-gatekeeper.github.io/)).

### Usage

Simply pass the path to one or more `.clap` plugins to `clap-validator validate`
to run the validator on those plugins. The `--only-failed` option can be used to
hide the output from all successful and skipped tests. Running `clap-validator
validate --help` lists all available options:

```shell
./clap-validator validate /path/to/the/plugin.clap --parallel
./clap-validator validate /path/to/the/plugin.clap --parallel --only-failed
./clap-validator validate --help
```

## Building

After installing [Rust](https://rustup.rs/), you can compile and run clap-validator as follows:

```shell
cargo run --release -- validate /path/to/the/plugin.clap
```
