# rucc-harness

The parts that the rucc test harnesses share. The harnesses build real C programs with [rucc](https://github.com/tamnd/rucc) and with a reference compiler, and compare the results. Each harness lives in its own repository: [rucc-postgres](https://github.com/tamnd/rucc-postgres), [rucc-kernel](https://github.com/tamnd/rucc-kernel) and [rucc-bsd](https://github.com/tamnd/rucc-bsd). This repository holds the code that they would otherwise copy from each other.

## Crates

- `harness-shim`: the compiler shim as a library. It classifies a compiler command line, runs the compiler, and appends one record per call to `compile.jsonl`. It also holds the file digests, the resource usage of the child, and the bring-up delegation rules.
- `harness-stats`: the statistics of a benchmark. Medians, quantiles, spreads, ratios, the geometric mean, a seeded bootstrap interval and the gate that turns a set of ratios into a verdict.
- `harness-elf`: summaries of ELF objects through the `object` crate. Sections, symbols, archive members and linker sets, and the difference between two objects.
- `harness-agent`: runs a list of jobs on a bounded pool, on this machine or on another one over ssh.

## Use

A harness depends on a tagged release by git:

```toml
[dependencies]
harness-shim = { git = "https://github.com/tamnd/rucc-harness", tag = "v0.1.0" }
```

## Build

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The toolchain is pinned in `rust-toolchain.toml`.

## License

Apache License 2.0. See `LICENSE`.
