# trcode-plugin-test

A test pane for [TerminalCode](https://github.com/Tim-kieboom/TerminalCode), as a plugin: a tree of tests, run all or one, results as they arrive. It knows no language. A **language adapter** (itself a plugin) does the discovering, running and parsing, and the test pane talks to it through plugin-to-plugin calls. Rust will ship as a bundled adapter.

Status: v0, the contract, and the Rust adapter (it runs real tests through `cargo nextest`, which must be installed; the pane tries it first, `t` or "Use Rust/Fake Adapter" choose). The pane shows the tests as folders (binary, modules, tests), `right`/`left` open and close them and `enter` runs a test or a folder. Two buttons are above them: **Find tests**, and **Run v** with a list of Run all, Run selected and Cancel run. See [docs/plan.md](docs/plan.md).

| crate | what it is |
|---|---|
| `crates/protocol` | the payloads of the calls between the pane and an adapter (`discover`, `run`, events) |
| `crates/fake-adapter` | an adapter with made-up tests and a clock you turn, to provoke what real ones do |
| `crates/rust-adapter` | the Rust adapter: `cargo nextest` to list and run, parsing what it prints |
| `crates/test-ui` | the throwaway pane |
| `crates/contract` | the stress tests, through a stand-in for the editor's plugin calls, and the fuel measurements |

```text
cargo test                                                    # the contract tests
cargo build --target wasm32-unknown-unknown --release         # the two modules
cargo test -p trcode-test-contract --test fuel -- --ignored   # what they cost in the editor's fuel
```

To try them in the editor (plugin API 1.8), put each module and its `manifest.toml` in a folder of the plugins folder (`fake-adapter` and `test-ui`), turn both on in "Plugins: Manage", and use the pane's actions (`d` discover, `a` run all, `n` turns the fake clock, `c` cancels).

Licensed under MIT or Apache-2.0.
