# cargo-buckal

Seamlessly build Cargo packages with Buck2.

![demo](docs/demo.gif)

## Install

```
cargo install --git https://github.com/buck2hub/cargo-buckal.git
```

## Usage

Run `cargo buckal --help` for full usage.

Common commands:

- `cargo buckal init`: initialize a Buck2-enabled workspace in an existing directory
- `cargo buckal migrate`: migrate an existing Cargo workspace to Buck2 (generate/update BUCK files)
- `cargo buckal build`: build the current package with Buck2
- `cargo buckal new|add|remove|update|autoremove`: manage Cargo dependencies
- `cargo buckal clean`: clean `buck-out` directory
- `cargo buckal version`: print version information

## Supported platforms

Platform-aware dependency mapping and bundled sample platforms currently target these Rust tier-1
host triples:

- Linux: `x86_64-unknown-linux-gnu`, `i686-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- Windows: `x86_64-pc-windows-msvc`, `x86_64-pc-windows-gnu`, `i686-pc-windows-msvc`, `aarch64-pc-windows-msvc`
- macOS: `aarch64-apple-darwin`

## Multi-platform builds

`cargo buckal migrate` preserves platform-conditional Cargo dependencies by emitting `os_deps`/`os_named_deps` and canonical OS constraints, so the same generated BUCK files can be built for different target platforms without regenerating on each host.

See [doc/multi-platform.md](doc/multi-platform.md).

## Configuration

You can configure cargo-buckal by creating a configuration file at `~/.config/buckal/config.toml`.

### Custom Buck2 Binary Path

If you have buck2 installed in a custom location, you can specify the path:

```toml
buck2_binary = "/path/to/your/buck2"
```

If no configuration file exists, cargo-buckal will use `buck2` (searches your PATH).
