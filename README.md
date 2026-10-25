# trcode-plugin-test

A test pane for [TerminalCode](https://github.com/Tim-kieboom/TerminalCode), as a plugin: a tree of tests, run all or one, results as they arrive. It knows no language. A **language adapter** (itself a plugin) does the discovering, running and parsing, and the test pane talks to it through plugin-to-plugin calls. Rust will ship as a bundled adapter.

Status: v0, the contract, and the Rust adapter (it runs real tests through `cargo nextest`, which must be installed; the pane tries it first, `t` or "Use Rust/Fake Adapter" choose). The pane shows the tests as folders (binary, modules, tests), `right`/`left` open and close them and `enter` runs a test or a folder. A click on a test opens its code in the editor (`g` does the same for the selected one): a test that failed goes to the line its panic names; any other is found by `grep` for its `fn` (Linux and macOS; on Windows the pane says there is no code to go to, since there is no grep). What a failed test wrote is shown under the tests while it is selected. While the tests are being looked for (the project is built first, which can take a while) the header and the place of the tests say so and the button reads Searching...; when the build fails the header says `the project does not build` and the compiler's errors are shown where the tests would be, all of them in a popup with **Show Problem** (`p`, or in the right-click menu). A **right click** on a test (or a folder) opens a menu: its code, its error, run it, debug it, find the tests again; **Show Test Error** (`e`) shows the whole error in a popup, and **Debug Selected** (`shift+d`) has the adapter build the test and the editor start it under the debugger (needs `lldb-dap`, and a trusted folder; the pane needs plugin API 1.11). Two buttons are above them: **Find tests**, and **Run v** with a list of Run all, Run selected and Cancel run. See [docs/plan.md](docs/plan.md).

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

To try them in the editor (plugin API 1.11), put each module and its `manifest.toml` in a folder of the plugins folder (`fake-adapter` and `test-ui`), turn both on in "Plugins: Manage", and use the pane's actions (`d` discover, `a` run all, `n` turns the fake clock, `c` cancels).

Licensed under MIT or Apache-2.0.
