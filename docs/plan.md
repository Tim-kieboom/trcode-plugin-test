# Plan

Decided in a grill session. Everything here is a decision unless it is under "Open".

## Shape

- **The test pane** is a plugin: the UI (tree, status, run, cancel, rerun) and nothing about any language. It holds no `process.run`.
- **A language adapter** is a plugin that owns discovery, execution and parsing, with its own `process.run` allowlist. The test pane never runs a command.
- **Rust** is a bundled adapter plugin on the same protocol as any later language (no special case in the test pane). It needs `cargo-nextest`; discovery is `cargo nextest list --message-format json` (documented), results are what `cargo nextest run` prints while it runs. Doctests are out of v1.
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
| Stalls | A slow adapter does not freeze the editor or other plugins | in the editor itself (`src/tests/real_test_plugin_tests.rs` in TerminalCode): a runaway adapter is stopped by the budget in about half a second and started again; an adapter busy for seconds on its own thread leaves the pane answering in milliseconds. The stand-in only checks the manifest asks for `thread = "own"` |
| Size | A 20,000-test discovery arrives in pages; the first renders before the last arrives; the pane draws a window | the same, plus `tests/fuel.rs` which runs the real modules in `wasmi` and measures the fuel |

The stand-in host (`crates/contract/src/lib.rs`) is a model of the editor's plugin calls, so it can drift from it; the same stresses (and the Rust adapter with the trust question) are also run on the real modules in the editor, by ignored tests in TerminalCode (`src/tests/real_test_plugin_tests.rs`, `TRCODE_PLUGIN_TEST_DIR=<this checkout>`).

### What v0 found

- **The flat budget broke the size stress.** The editor gives a plugin 5 million fuel for each message it handles (`CALLBACK_FUEL`). A page of 500 tests cost the fake adapter, which only formats names, 7.5 million: it would have been stopped as a runaway. Pages are 100 tests now (`PAGE`): the adapter uses 1.6 million and the pane taking the page 3.1 million. A real adapter parsing `cargo nextest list` output does far more per test, and its listing is one big JSON object that cannot be paged by the adapter after the fact, so **the editor's budget has to grow with what a plugin on its own thread is sent** (today only the text of a file does), for the payload of a `Reply` and the text of a `RunOutput` at least. That is a change in TerminalCode (`fuel_for` in `src/plugins/runtime.rs`): made, in plugin API 1.8 (the payload of a `Call` or `Reply` and the text of a `RunOutput` are charged like the text of a file, on a thread of the plugin's own). It cannot help work that is output (an adapter making a page), so pages stay at 100.
- **Discovery is pulled, a page a call**, not streamed: an adapter cannot say 40 pages in one handle without the budget stopping it.
- **The pane needs a thread of its own.** In the editor, pulling 20,000 tests ran the pane out of the flat budget of the shared thread (its table grows, and a page then costs more than the 3 million measured at a small size), so its manifest has `thread = "own"`, where the budget grows with the page. The fake adapter's `burns` loop had been folded into a sum by the compiler, so its slow-work scenarios did nothing until the editor ran them.
- **Everything the pane does for 20,000 tests fits** (run all 0.9 million, an event 0.3, a key 0.25, cancel 1.1), but `advance` for 50 tests is already half the budget: a real adapter must not do a test's worth of work per message.

- **A plugin without a pane.** The adapter asked where its pane goes and has none. `view = false` in the manifest (plugin API 1.8) says so: no question, no place, what it draws is dropped.

## The tests as folders (built in the throwaway pane)

The pane shows the tests as a tree like the testing view of an editor that has one: a folder for the binary (the `group` the adapter gives), a folder for each module of the name (cut at `::`), a test for each name. `v`/`>` says a folder is open or closed, and a folder says how its tests are doing (`[ok]`, `[FAIL]`, `[..]`, `[?]` for the worst of what is in it) and how many are done, `(3/5)`. `right` opens a folder (or goes into an open one), `left` closes it (or goes to the folder a test is in), `up` and `down` visit what is shown, a click selects and opens or closes a folder, and `enter` runs the selected test or every test in the selected folder (everything, when the folder has every test). Above the tests are buttons: **Find tests** asks the adapter for the tests again, and **Run v** opens a list of every way to run (Run all, Run selected, Cancel run; each is dim when it cannot be done, and choosing one does it and closes the list). The editor has no drop-down view, so the list is a second row of buttons that the Run button opens and closes (`^`/`v` show which); each row of buttons takes a line of the pane (more when they do not fit its width), which the pane takes from the tests it shows. A project of up to 200 tests starts open; a bigger one starts closed. The path comes from the name and the group, so an adapter for another language gives the same tree by naming its tests `module::test` (a Python adapter would say `Class::test` and put the file in the group); an explicit path in `TestItem` is the next step if that is not enough.

What it costs in the editor's fuel with 20,000 tests: a key 0.07 million, an event 0.1, run all or cancel about 2 (the folders are counted again once, not for each test; counting each test's folders one by one cost 3.5 and 3.9, near the flat 5 that a message without a big payload gets).

## The Rust adapter (built, v1)

`crates/rust-adapter`, the plugin `rust-adapter` (no pane, its own thread, `process.run` for `cargo nextest list *` and `cargo nextest run *`, the folder must be trusted).

- **Results come from nextest's run output, not JUnit.** Tried on a real project: nextest writes JUnit only when the project's own `.config/nextest.toml` names a path (there is no default file), and a plugin cannot read a file anyway; its JSON for a run (`libtest-json`) is experimental, needs an environment variable, and names tests `fx::fx$tests::name`, which does not map back to a binary id. The lines it prints while it runs (`PASS [ 0.003s] (1/5) fx::bin/fx tests::parses_empty`, a failed test's output after its line) stream live, need no setup, and carry the binary id. The format is not documented, so the parser is tested against what nextest really prints (`tests/fixtures`, and nextest itself when it is installed, in `src/tests/real.rs` and the contract tests). Only nextest's stderr is read; `--color never` keeps it plain.
- **Identity is `binary-id` + a space + the test name** (neither has a space), which is also what the filter is made of: `(binary_id(=fx::bin/fx) & test(=tests::parses_empty))`, tests of one binary joined with `|`. A name with more than letters, digits and `_:/-.` is matched as a whole by a regex (`/^a\(1\)$/`), because nextest takes a quote after `=` as part of the name; not tried on a real test with such a name (Rust function names cannot have one).
- **The list is read as it comes** by a scanner that keeps only suites, test names and the ignored flag (`list.rs`): the document is megabytes for a big project, a plugin has 16 MiB, and each piece the editor sends gets a budget that fits that piece. Measured in the editor's fuel: a 64 KiB piece of a 20,000-test listing uses 340 million of the 1.09 billion it is given; the end of the list and the first page 1.5 million of the flat 5; 64 KiB of run output 82 million. Ignored tests are not listed (nextest does not run them).
- **A run tells each test as its line comes** (a `Started` and a `Finished` together, since nextest does not say when a test starts), the failed test's message from where it panicked, and ends with `Done`. A run that ran nothing says why in an `Error` (no match, does not build, with the end of what cargo said). Exit code 100 (tests failed) is not an error.
- **Cancel** sends `RunCancel`, and the editor stops cargo and what it started.
- **Not done:** doctests; a `Started` that is real (nextest has no line for it); the number of tests running; test output of tests that passed; a filter for names that need the regex form, tried for real; the pane choosing its adapter by itself (it has an action to switch between the fake and the Rust adapter); a list of 100,000 tests (the adapter holds the list in memory).

## Order

1. Plugin-to-plugin messaging (host). Done.
2. This repo: fake adapter, throwaway UI, the stress tests. Done, but for death and stalls in the real editor, and the budget that grows (above).
3. `process.run`, allowlist, workspace trust (host).
4. The Rust adapter (bundled). Built, not released; the bundling is still open.
5. The real UI.
6. Coverage.

## Coverage (later)

- Per-test coverage through `cargo llvm-cov nextest` with a `LLVM_PROFILE_FILE` per test (nextest runs each test in its own process).
- **Bad path = tests that expect failure**, marked by an **explicit tag** on the test (syntax open). Tests without a tag are shown as **unclassified**, a category of their own; they are never counted as good path.

## Open

- **Reading nextest's JUnit file.** `process.run` returns output, not files, and nextest writes JUnit XML to a file. Options: a read-only project-scoped `files.read` capability (plugin API 1.8), an allowlisted `cat`, or the experimental libtest-json on stdout. Decide before the Rust adapter.
- `trcode-plugin-api` is a git dependency on the tag `plugin-api-1.8` of TerminalCode (the commit of that tag is not on its `main` yet). CI runs fmt, clippy, the tests (with nextest installed, so the real-nextest tests run), the WebAssembly build and the fuel measurements.

- How the plugin-to-plugin messages are versioned.
- The tag syntax for a bad-path test.
- How a bundled adapter is shipped and installed.
- Verify against real nextest output that JUnit `classname` is the binary id.

## Going to the code of a test, and seeing why it failed

- **A click opens the code.** The pane asks the editor to open a file at a line (plugin API 1.9, `open_file` with `line` and `column`; the pane needs `editor.open`). The place of a test that failed comes with its result (`Finished.location`, read by the adapter from the panic line of its message: `src/lib.rs:14:9`; a path outside the project is no place). Any other test is asked of the adapter with the call `locate`.
- **`locate` in the Rust adapter** runs `grep -rnwF -I --exclude-dir=target --exclude-dir=.git -e "fn name" .` in the project (the manifest allows exactly that) and keeps the best line. nextest does not say where a test is, so this is a guess that is right in almost every project: the binary id tells the file (`fx::it` is `tests/it.rs`, the library prefers `src/lib.rs`, a binary `src/main.rs`) and the modules of the name count for the folders and the file of the match. Not found, no grep (Windows) and a grep that failed are each said in the pane's header.
- **The message** of the selected test that failed (the text it wrote, from where it panicked) is shown under the tests, at most a third of the pane, each line cut to the width, with how many lines were left out. A new run forgets it.
- **Not done:** the place of a test on Windows without grep (a `files.find` in the editor would give it on every system); a test inside a `macro_rules!` or one whose `fn` is not on one line with its name; doctests.
