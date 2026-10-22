//! A small stand-in for the editor's plugin calls, to run real plugins against: it
//! passes messages between them the way the editor does (the rules are in "Plugins
//! that run tests" in TerminalCode's `docs/design.md`) and counts a panic as the trap
//! of a WebAssembly module.
//!
//! It is a model, so it can drift from the editor; what it cannot show (a budget that
//! stops a loop, a thread of its own) is for the tests that run the real modules in the
//! editor.

use std::any::Any;
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use trcode_plugin_api::{
    Capability, FromPlugin, MAX_OPEN_CALLS, Plugin, Refusal, ToPlugin, WireView,
};

/// A plugin the host can also be asked about: its own type.
trait Hosted: Plugin {
    fn as_any(&self) -> &dyn Any;
}

impl<P: Plugin + 'static> Hosted for P {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct Slot {
    plugin: Box<dyn Hosted>,
    /// Makes the plugin again, as the editor does after a trap.
    make: fn() -> Box<dyn Hosted>,
    may_call: bool,
    serves: bool,
}

/// A message that was delivered to a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub to: String,
    pub message: ToPlugin,
}

#[derive(Default)]
pub struct MiniHost {
    slots: BTreeMap<String, Slot>,
    /// The calls that are open: caller and its request number, to the callee.
    open: BTreeMap<(String, u64), String>,
    queue: VecDeque<(String, ToPlugin)>,
    views: BTreeMap<String, WireView>,
    delivered: Vec<Delivered>,
    deaths: Vec<String>,
}

thread_local! {
    /// Whether a plugin is running on this thread: its panic is a trap, not news.
    static IN_PLUGIN: Cell<bool> = const { Cell::new(false) };
}

/// Silences the report of a panic inside a plugin (a trap is expected), and only that.
fn quiet_traps() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let reporting = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !IN_PLUGIN.get() {
                reporting(info);
            }
        }));
    });
}

impl MiniHost {
    pub fn new() -> Self {
        quiet_traps();
        Self::default()
    }

    /// Adds a plugin with what its manifest asks for, and starts it.
    pub fn add<P: Plugin + Default + 'static>(&mut self, id: &str, capabilities: &[Capability]) {
        let slot = Slot {
            plugin: Box::new(P::default()),
            make: || Box::new(P::default()),
            may_call: capabilities.contains(&Capability::PluginCall),
            serves: capabilities.contains(&Capability::PluginServe),
        };
        self.slots.insert(id.to_owned(), slot);
        self.queue.push_back((id.to_owned(), ToPlugin::Start));
    }

    /// Tells a plugin something, as the editor would (a key, a size), and lets everything
    /// that follows happen.
    pub fn send(&mut self, to: &str, message: ToPlugin) {
        self.queue.push_back((to.to_owned(), message));
        self.run_until_quiet();
    }

    /// Lets everything that is waiting happen.
    pub fn run_until_quiet(&mut self) {
        while self.step() {}
    }

    /// One delivery. `false` when nothing was waiting.
    pub fn step(&mut self) -> bool {
        let Some((to, message)) = self.queue.pop_front() else {
            return false;
        };
        if !self.slots.contains_key(&to) {
            return true;
        }
        self.delivered.push(Delivered {
            to: to.clone(),
            message: message.clone(),
        });
        let slot = self.slots.get_mut(&to).expect("checked above");
        IN_PLUGIN.set(true);
        let outcome = catch_unwind(AssertUnwindSafe(|| slot.plugin.handle(message)));
        IN_PLUGIN.set(false);
        match outcome {
            Ok(said) => {
                for message in said {
                    self.route(&to, message);
                }
            }
            Err(_) => self.die(&to),
        }
        true
    }

    /// Makes the plugin `id` trap, as a module does that runs out of budget or hits a bug.
    pub fn kill(&mut self, id: &str) {
        self.die(id);
        self.run_until_quiet();
    }

    /// What the plugin `id` last drew.
    pub fn view(&self, id: &str) -> Option<&WireView> {
        self.views.get(id)
    }

    /// The lines of what the plugin `id` last drew, as text.
    pub fn lines(&self, id: &str) -> Vec<String> {
        match self.views.get(id) {
            Some(WireView::Rows { lines, .. } | WireView::Lines { lines }) => {
                lines.iter().map(|line| line.text.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The plugin `id`, as the type it is.
    pub fn plugin<P: 'static>(&self, id: &str) -> Option<&P> {
        self.slots.get(id)?.plugin.as_any().downcast_ref()
    }

    /// Every message delivered so far, in order.
    pub fn delivered(&self) -> &[Delivered] {
        &self.delivered
    }

    /// The plugins that trapped, in order.
    pub fn deaths(&self) -> &[String] {
        &self.deaths
    }

    /// How many calls are open.
    pub fn open_calls(&self) -> usize {
        self.open.len()
    }

    fn route(&mut self, from: &str, message: FromPlugin) {
        match message {
            FromPlugin::SetView { view } => {
                self.views.insert(from.to_owned(), view);
            }
            FromPlugin::Call {
                request,
                to,
                method,
                payload,
            } => self.call(from, request, to, method, payload),
            FromPlugin::Reply {
                to,
                request,
                payload,
                last,
            } => {
                let key = (to.clone(), request);
                let serves = self.slots.get(from).is_some_and(|slot| slot.serves);
                // A part of an answer to a call that is not open with this plugin is dropped.
                if serves && self.open.get(&key).is_some_and(|callee| callee == from) {
                    if last {
                        self.open.remove(&key);
                    }
                    let reply = ToPlugin::Reply {
                        from: from.to_owned(),
                        request,
                        payload,
                        last,
                    };
                    self.queue.push_back((to, reply));
                }
            }
            FromPlugin::CallCancel { request } => {
                if let Some(callee) = self.open.remove(&(from.to_owned(), request)) {
                    let cancelled = ToPlugin::CallCancelled {
                        from: from.to_owned(),
                        request,
                    };
                    self.queue.push_back((callee, cancelled));
                }
            }
            _ => {}
        }
    }

    fn call(&mut self, from: &str, request: u64, to: String, method: String, payload: String) {
        let refuse = |reason| ToPlugin::Refused { request, reason };
        let may_call = self.slots.get(from).is_some_and(|slot| slot.may_call);
        let open_here = self
            .open
            .keys()
            .filter(|(caller, _)| caller == from)
            .count();
        let refusal = if !may_call {
            Some(Refusal::Denied {
                capability: Capability::PluginCall,
            })
        } else if to == from || self.open.contains_key(&(from.to_owned(), request)) {
            Some(Refusal::BadArgument {
                detail: "a call to itself, or a number that is open".to_owned(),
            })
        } else if !self.slots.get(&to).is_some_and(|slot| slot.serves) {
            Some(Refusal::NoSuchPlugin)
        } else if open_here >= MAX_OPEN_CALLS {
            Some(Refusal::Busy)
        } else {
            None
        };
        if let Some(reason) = refusal {
            self.queue.push_back((from.to_owned(), refuse(reason)));
            return;
        }
        self.open.insert((from.to_owned(), request), to.clone());
        let call = ToPlugin::Call {
            from: from.to_owned(),
            request,
            method,
            payload,
        };
        self.queue.push_back((to, call));
    }

    /// The plugin trapped: it starts again from nothing, the calls it was answering fail,
    /// and the calls it made are over.
    fn die(&mut self, id: &str) {
        self.deaths.push(id.to_owned());
        let answering: Vec<(String, u64)> = self
            .open
            .iter()
            .filter(|(_, callee)| callee.as_str() == id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in answering {
            self.open.remove(&key);
            let gone = ToPlugin::Refused {
                request: key.1,
                reason: Refusal::Gone,
            };
            self.queue.push_back((key.0, gone));
        }
        let made: Vec<(String, u64)> = self
            .open
            .keys()
            .filter(|(caller, _)| caller == id)
            .cloned()
            .collect();
        for key in made {
            if let Some(callee) = self.open.remove(&key) {
                let cancelled = ToPlugin::CallCancelled {
                    from: id.to_owned(),
                    request: key.1,
                };
                self.queue.push_back((callee, cancelled));
            }
        }
        if let Some(slot) = self.slots.get_mut(id) {
            slot.plugin = (slot.make)();
        }
        self.queue.push_back((id.to_owned(), ToPlugin::Start));
    }
}
