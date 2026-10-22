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

## Host work (TerminalCode repository, ships first): built, not released yet

1. Plugin-to-plugin messaging in `trcode-plugin-api` (plugin API 1.6): `Call`/`Reply`/`CallCancel`, capabilities `plugin.call` and `plugin.serve`, `payload` is opaque text. Replies are dropped unless their call is open, so a cancel is final; a callee that traps or goes fails its calls with `Gone`.
2. `process.run` capability (plugin API 1.7): `Run`/`RunCancel`, streamed `RunOutput`, `RunDone`, `Trust`; a manifest allowlist of program plus argument pattern (`cargo nextest *`), shown at install like the other capabilities, **and refused until the workspace is trusted** (the trust prompt says plainly that running tests executes code of the open project: `build.rs`, proc-macros, `conftest.py`).

## v0: prove the contract (built)

A **fake adapter** and a **throwaway pane**, plugin-to-plugin calls only: no process spawning, no trust. The fake adapter makes up its tests and runs them by a clock the caller turns (`advance`), so a cancel in the middle of a run, an adapter that talks after a cancel or dies, tests with one name, and twenty thousand tests can be made to happen on purpose. A scenario (`configure`) says which.

| Stress | Pass condition | Where it is tested |
|---|---|---|
| Ids | Two tests with the same name in different binaries are two rows; re-running one runs only that one | `crates/contract/tests/contract.rs` |
| Cancel | Cancel then an immediate re-run shows only the second run's results | the same, with an adapter that keeps talking after the cancel; and the pane ignores an event of another run even if one gets through |
| Death | If the adapter traps mid-run, "running" tests become "unknown" at once and the next run works | the same; and a pane that dies cancels its run |
| Stalls | A slow adapter does not freeze the editor or other plugins | **not yet**: the stand-in has no threads. The manifest asks for `thread = "own"` (tested); the budget stopping a loop needs a test that runs the module in the editor |
| Size | A 20,000-test discovery arrives in pages; the first renders before the last arrives; the pane draws a window | the same, plus `tests/fuel.rs` which runs the real modules in `wasmi` and measures the fuel |

The stand-in host (`crates/contract/src/lib.rs`) is a model of the editor's plugin calls, so it can drift from it; the tests that run these modules in the editor itself (death and stalls for real) are still to write, in TerminalCode like the Git pane's `real_plugin_tests`.

### What v0 found

- **The flat budget broke the size stress.** The editor gives a plugin 5 million fuel for each message it handles (`CALLBACK_FUEL`). A page of 500 tests cost the fake adapter, which only formats names, 7.5 million: it would have been stopped as a runaway. Pages are 100 tests now (`PAGE`): the adapter uses 1.6 million and the pane taking the page 3.1 million. A real adapter parsing `cargo nextest list` output does far more per test, and its listing is one big JSON object that cannot be paged by the adapter after the fact, so **the editor's budget has to grow with what a plugin on its own thread is sent** (today only the text of a file does), for the payload of a `Reply` and the text of a `RunOutput` at least. That is a change in TerminalCode (`fuel_for` in `src/plugins/runtime.rs`) to make before the Rust adapter.
- **Discovery is pulled, a page a call**, not streamed: an adapter cannot say 40 pages in one handle without the budget stopping it.
- **Everything the pane does for 20,000 tests fits** (run all 0.9 million, an event 0.3, a key 0.25, cancel 1.1), but `advance` for 50 tests is already half the budget: a real adapter must not do a test's worth of work per message.

## Order

1. Plugin-to-plugin messaging (host). Done.
2. This repo: fake adapter, throwaway UI, the stress tests. Done, but for death and stalls in the real editor, and the budget that grows (above).
3. `process.run`, allowlist, workspace trust (host).
4. The Rust adapter (bundled).
5. The real UI.
6. Coverage.

## Coverage (later)

- Per-test coverage through `cargo llvm-cov nextest` with a `LLVM_PROFILE_FILE` per test (nextest runs each test in its own process).
- **Bad path = tests that expect failure**, marked by an **explicit tag** on the test (syntax open). Tests without a tag are shown as **unclassified**, a category of their own; they are never counted as good path.

## Open

- **Reading nextest's JUnit file.** `process.run` returns output, not files, and nextest writes JUnit XML to a file. Options: a read-only project-scoped `files.read` capability (plugin API 1.8), an allowlisted `cat`, or the experimental libtest-json on stdout. Decide before the Rust adapter.
- This repo depends on `trcode-plugin-api` by a path (`../TerminalCode`) until the tag `plugin-api-1.7` is on GitHub; then a git dependency, and CI (it was left out: it cannot check out the editor next to itself).

- How the plugin-to-plugin messages are versioned.
- The tag syntax for a bad-path test.
- How a bundled adapter is shipped and installed.
- Verify against real nextest output that JUnit `classname` is the binary id.
