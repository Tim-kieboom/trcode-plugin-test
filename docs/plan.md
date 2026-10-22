# Plan

Decided in a grill session. Everything here is a decision unless it is under "Open".

## Shape

- **The test pane** is a plugin: the UI (tree, status, run, cancel, rerun) and nothing about any language. It holds no `process.run`.
- **A language adapter** is a plugin that owns discovery, execution and parsing, with its own `process.run` allowlist. The test pane never runs a command.
- **Rust** is a bundled adapter plugin on the same protocol as any later language (no special case in the test pane). It needs `cargo-nextest`; results come from nextest's JUnit XML (stable, written when the run ends), discovery from `cargo nextest list --message-format json`. Doctests are out of v1.
- The host-visible contract is typed messages, not JUnit. JUnit is the Rust adapter's own detail; another adapter may stream events.

## Protocol (plugin to plugin, through the host)

- `discover` (paged, at most 500 per page), `run { ids }`, `cancel`.
- Events carry a `run_id`; the host and the UI drop events of any other run.
- A test id is opaque and assigned by the adapter. The UI never parses names. A name can repeat across binaries (nextest's `classname` is the binary id, `name` the test path in it, so the pair is the identity inside the Rust adapter).
- Adapters declare `thread = "own"`.

## Host work (TerminalCode repository, ships first)

1. Plugin-to-plugin messaging in `trcode-plugin-api` (a minor ABI bump, additive).
2. `process.run` capability: a manifest allowlist of program plus argument pattern (`cargo nextest *`), shown at install like the other capabilities, **and refused until the workspace is trusted** (the trust prompt says plainly that running tests executes code of the open project: `build.rs`, proc-macros, `conftest.py`).

## v0: prove the contract

A **fake adapter** and a **throwaway UI**, plugin-to-plugin calls only: no process spawning, no trust. Done when these pass as automated tests against the fake adapter (the same suite is the contract test any real adapter must pass):

| Stress | Pass condition |
|---|---|
| Ids | Two tests with the same name in different binaries are two rows; re-running one runs only that one |
| Cancel | Cancel then an immediate re-run shows only the second run's results |
| Death | If the adapter traps mid-run, "running" tests become "unknown (adapter died)" within 1 s and the next run works |
| Stalls | A 5 s sleep in the adapter does not freeze the editor or other plugins |
| Size | A 20,000-test discovery arrives in pages of at most 500; the first page renders before the last arrives |

## Order

1. Plugin-to-plugin messaging (host).
2. This repo: fake adapter, throwaway UI, the five tests.
3. `process.run`, allowlist, workspace trust (host).
4. The Rust adapter (bundled).
5. The real UI.
6. Coverage.

## Coverage (later)

- Per-test coverage through `cargo llvm-cov nextest` with a `LLVM_PROFILE_FILE` per test (nextest runs each test in its own process).
- **Bad path = tests that expect failure**, marked by an **explicit tag** on the test (syntax open). Tests without a tag are shown as **unclassified**, a category of their own; they are never counted as good path.

## Open

- How the plugin-to-plugin messages are versioned.
- The tag syntax for a bad-path test.
- How a bundled adapter is shipped and installed.
- Verify against real nextest output that JUnit `classname` is the binary id.
