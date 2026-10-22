//! What the real modules cost in the editor's fuel. The editor gives each message a plugin
//! handles a flat budget (`CALLBACK_FUEL` in TerminalCode's `src/plugins/mod.rs`), and a
//! plugin that goes over it is stopped as a runaway, so the size of a page and the work
//! the pane does for twenty thousand tests have to fit in it.
//!
//! These need the modules built, so they are ignored unless asked for:
//!
//! ```text
//! cargo build --target wasm32-unknown-unknown --release
//! cargo test -p trcode-test-contract --test fuel -- --ignored --nocapture
//! ```

use trcode_plugin_api::{FromPlugin, ToPlugin};
use trcode_test_protocol::{
    AdvanceArgs, DiscoverArgs, PAGE, Page, RunArgs, RunEvent, Scenario, Status, method, to_payload,
};
use wasmi::{Caller, Config, Engine, Extern, Linker, Memory, Module, Store, TypedFunc};

/// The budget of one message on the editor's shared thread, and the same on a thread of a
/// plugin's own unless it holds the text of a file.
const BUDGET: u64 = 5_000_000;

struct Sent(Vec<Vec<u8>>);

struct Module_ {
    store: Store<Sent>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    handle: TypedFunc<(i32, i32), ()>,
}

fn load(name: &str) -> Module_ {
    let path = format!(
        "{}/../../target/wasm32-unknown-unknown/release/{name}.wasm",
        env!("CARGO_MANIFEST_DIR")
    );
    let wasm = std::fs::read(&path).unwrap_or_else(|error| {
        panic!("{path}: {error}; build with --target wasm32-unknown-unknown --release")
    });
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, &wasm).unwrap();
    let mut store = Store::new(&engine, Sent(Vec::new()));
    store.set_fuel(u64::MAX).unwrap();
    let mut linker = <Linker<Sent>>::new(&engine);
    linker
        .func_wrap(
            "host",
            "send",
            |mut caller: Caller<'_, Sent>, ptr: i32, len: i32| {
                let memory = caller
                    .get_export("memory")
                    .and_then(Extern::into_memory)
                    .unwrap();
                let mut bytes = vec![0; len as usize];
                memory.read(&caller, ptr as usize, &mut bytes).unwrap();
                caller.data_mut().0.push(bytes);
            },
        )
        .unwrap();
    let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
    Module_ {
        memory: instance.get_memory(&store, "memory").unwrap(),
        alloc: instance.get_typed_func(&store, "alloc").unwrap(),
        handle: instance.get_typed_func(&store, "handle").unwrap(),
        store,
    }
}

impl Module_ {
    /// Hands the module a message; what it said, and the fuel the call took.
    fn deliver(&mut self, message: &ToPlugin) -> (Vec<FromPlugin>, u64) {
        let bytes = message.to_bytes().unwrap();
        let len = bytes.len() as i32;
        self.store.set_fuel(BUDGET * 100).unwrap();
        let ptr = self.alloc.call(&mut self.store, len).unwrap();
        self.memory
            .write(&mut self.store, ptr as usize, &bytes)
            .unwrap();
        self.handle.call(&mut self.store, (ptr, len)).unwrap();
        let used = BUDGET * 100 - self.store.get_fuel().unwrap();
        let said = std::mem::take(&mut self.store.data_mut().0)
            .iter()
            .map(|bytes| FromPlugin::from_bytes(bytes).unwrap())
            .collect();
        (said, used)
    }
}

fn call(request: u64, name: &str, payload: String) -> ToPlugin {
    ToPlugin::Call {
        from: "test-ui".into(),
        request,
        method: name.into(),
        payload,
    }
}

fn replies_of(said: &[FromPlugin]) -> Vec<(u64, String, bool)> {
    said.iter()
        .filter_map(|message| match message {
            FromPlugin::Reply {
                request,
                payload,
                last,
                ..
            } => Some((*request, payload.clone(), *last)),
            _ => None,
        })
        .collect()
}

fn report(what: &str, used: u64) {
    println!("{what}: {used} fuel, {}% of {BUDGET}", used * 100 / BUDGET);
    assert!(used < BUDGET, "{what} would be stopped as a runaway");
}

/// A pane that has been told about `tests` tests, as pages came from an adapter.
fn pane_with(tests: usize) -> Module_ {
    let mut pane = load("trcode_test_ui");
    pane.deliver(&ToPlugin::Start);
    let mut adapter = load("trcode_test_fake_adapter");
    adapter.deliver(&call(
        0,
        method::CONFIGURE,
        to_payload(&Scenario {
            tests,
            ..Scenario::default()
        }),
    ));
    for (request, offset) in (1..).zip((0..tests).step_by(PAGE)) {
        let (said, _) = adapter.deliver(&call(
            request,
            method::DISCOVER,
            to_payload(&DiscoverArgs::page_at(offset)),
        ));
        let (_, payload, _) = replies_of(&said).remove(0);
        pane.deliver(&ToPlugin::Reply {
            from: "fake-adapter".into(),
            request,
            payload,
            last: true,
        });
    }
    pane
}

#[test]
#[ignore = "needs the modules built"]
fn a_page_of_tests_fits_the_budget_on_both_sides() {
    let mut adapter = load("trcode_test_fake_adapter");
    adapter.deliver(&call(
        0,
        method::CONFIGURE,
        to_payload(&Scenario {
            tests: 20_000,
            ..Scenario::default()
        }),
    ));
    let (said, made) = adapter.deliver(&call(
        1,
        method::DISCOVER,
        to_payload(&DiscoverArgs::page_at(0)),
    ));
    report("the adapter makes a page", made);

    let (_, payload, _) = replies_of(&said).remove(0);
    let page: Page = serde_json::from_str(&payload).unwrap();
    assert_eq!(page.tests.len(), PAGE);
    let mut pane = load("trcode_test_ui");
    pane.deliver(&ToPlugin::Start);
    let (_, taken) = pane.deliver(&ToPlugin::Reply {
        from: "fake-adapter".into(),
        request: 1,
        payload,
        last: true,
    });
    report("the pane takes a page", taken);
}

#[test]
#[ignore = "needs the modules built"]
fn the_work_of_the_pane_for_twenty_thousand_tests_fits_the_budget() {
    let mut pane = pane_with(20_000);

    let (_, used) = pane.deliver(&ToPlugin::action("run_all"));
    report("run all of 20000 (every test is queued)", used);

    let run = RunArgs {
        run_id: 1,
        ids: None,
    };
    assert_eq!(run.run_id, 1);
    let event = RunEvent::Finished {
        run_id: 1,
        id: "t19999".into(),
        status: Status::Passed,
        duration_ms: 1,
        message: String::new(),
    };
    let (_, used) = pane.deliver(&ToPlugin::Reply {
        from: "fake-adapter".into(),
        request: 21,
        payload: to_payload(&event),
        last: false,
    });
    report("one event while 20000 are loaded", used);

    let (_, used) = pane.deliver(&ToPlugin::action("down"));
    report("a key while 20000 are loaded", used);

    let (_, used) = pane.deliver(&ToPlugin::action("cancel"));
    report("cancel with 20000 queued", used);
}

#[test]
#[ignore = "needs the modules built"]
fn a_turn_of_the_fake_clock_for_a_few_tests_fits_the_budget() {
    let mut adapter = load("trcode_test_fake_adapter");
    adapter.deliver(&call(
        0,
        method::CONFIGURE,
        to_payload(&Scenario {
            tests: 20_000,
            ..Scenario::default()
        }),
    ));
    adapter.deliver(&call(
        1,
        method::RUN,
        to_payload(&RunArgs {
            run_id: 1,
            ids: None,
        }),
    ));

    let (_, used) = adapter.deliver(&call(
        2,
        method::ADVANCE,
        to_payload(&AdvanceArgs { n: 50 }),
    ));

    report("advance 50 of 20000", used);
}
