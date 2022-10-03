# clap-validator

[![Automated builds](https://github.com/robbert-vdh/clap-validator/actions/workflows/build.yml/badge.svg?branch=master)](https://github.com/robbert-vdh/clap-validator/actions/workflows/build.yml?query=branch%3Amaster)

A validator and automatic test suite for [CLAP](https://github.com/free-audio/clap) plugins. Clap-validator can automatically test one or more plugins for common bugs and incorrect behavior.

<!-- TODO: More usage instructions -->

## Download

Prebuilt binaries can be found
[here](https://nightly.link/robbert-vdh/clap-validator/workflows/build/master).

## Building

After installing [Rust](https://rustup.rs/), you can compile and run clap-validator as follows:

```shell
cargo run --release
```

If you are on an ARM mac you need to specify the target to ARM most likely.
For instance, here's how to validate `Surge XT` if you have an ARM-only
local build

```
cargo run --target aarch64-apple-darwin validate ~/Library/Audio/Plug-Ins/CLAP/Surge\ XT.clap
```

