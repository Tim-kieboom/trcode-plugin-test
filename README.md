# trcode-plugin-test

A test pane for [TerminalCode](https://github.com/Tim-kieboom/TerminalCode), as a plugin: a tree of tests, run all or one, results as they arrive. It knows no language. A **language adapter** (itself a plugin) does the discovering, running and parsing, and the test pane talks to it through plugin-to-plugin messages. Rust ships as a bundled adapter.

Status: planning. See [docs/plan.md](docs/plan.md).

Licensed under MIT or Apache-2.0.
