//! The tests as a tree, like the testing view of an editor that has one: a folder for the binary
//! (or the file), a folder for each module of the name, and a test for each name. Folders can be
//! open or closed and say how the tests in them are doing.
//!
//! The path of a test is made from what the adapter said: its group, then the name cut at `::`
//! (`tests::parses_empty` is the test `parses_empty` in the folder `tests`). A name without `::` is
//! a test directly in the folder of its group.
//!
//! Everything a key or an event does here is about the one node or a short walk from it (the
//! depth of the tree, or the height of the pane), never the whole tree: it can have tens of
//! thousands of tests and a plugin has a small budget for each message.

use std::collections::HashMap;

use crate::ui::RowStatus;

/// The number of states a test can be in.
pub(crate) const STATES: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Folder {
        open: bool,
    },
    /// The test is this row of the list of tests.
    Test {
        row: usize,
    },
}

#[derive(Debug)]
pub(crate) struct Node {
    pub name: String,
    pub parent: Option<usize>,
    /// Where it is among the nodes of its folder.
    pub index: usize,
    pub children: Vec<usize>,
    pub kind: Kind,
    /// How many of the tests in this folder are in each state (not kept for a test).
    pub counts: [usize; STATES],
    /// How many tests are in this node.
    pub tests: usize,
}

impl Node {
    pub(crate) fn is_folder(&self) -> bool {
        matches!(self.kind, Kind::Folder { .. })
    }

    /// How the tests in it are doing, as the mark of a folder.
    pub(crate) fn status(&self) -> RowStatus {
        let of = |status: RowStatus| self.counts[status.index()];
        if of(RowStatus::Failed) > 0 {
            RowStatus::Failed
        } else if of(RowStatus::Running) > 0 {
            RowStatus::Running
        } else if of(RowStatus::Queued) > 0 {
            RowStatus::Queued
        } else if of(RowStatus::Unknown) > 0 {
            RowStatus::Unknown
        } else if of(RowStatus::Passed) + of(RowStatus::Skipped) == self.tests && self.tests > 0 {
            RowStatus::Passed
        } else {
            RowStatus::Unrun
        }
    }
}

/// The name of a test as a folder shows it: what is after the last `::` (the rest are folders).
pub(crate) fn leaf(name: &str) -> &str {
    name.rsplit_once("::").map_or(name, |(_, leaf)| leaf)
}

#[derive(Debug, Default)]
pub(crate) struct Tree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    /// The folder with this name in this folder (`None`: at the top).
    folders: HashMap<(Option<usize>, String), usize>,
    /// The node of each test, by its row.
    test_nodes: Vec<usize>,
    /// The folders, in the order they were made (each before what is in it).
    folder_ids: Vec<usize>,
}

impl Tree {
    pub(crate) fn node(&self, id: usize) -> &Node {
        &self.nodes[id]
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The first node that is shown, if there is one.
    pub(crate) fn first(&self) -> Option<usize> {
        self.roots.first().copied()
    }

    /// Puts the test (row `row`, not run yet) in its place, making the folders it needs; they
    /// start `open` or not.
    pub(crate) fn add(&mut self, row: usize, group: &str, name: &str, open: bool) {
        let mut folders: Vec<&str> = Vec::new();
        if !group.is_empty() {
            folders.push(group);
        }
        let leaf = match name.rsplit_once("::") {
            Some((modules, leaf)) => {
                folders.extend(modules.split("::"));
                leaf
            }
            None => name,
        };
        let mut parent = None;
        for folder in folders {
            parent = Some(self.folder(parent, folder, open));
        }
        // A test does not keep its name here: the table has it, and a name is a piece of memory
        // that dropping the tree would have to free.
        let _ = leaf;
        let node = self.push(parent, "", Kind::Test { row });
        debug_assert_eq!(row, self.test_nodes.len());
        self.test_nodes.push(node);
        self.nodes[node].tests = 1;
        self.count(parent, RowStatus::Unrun.index());
    }

    fn folder(&mut self, parent: Option<usize>, name: &str, open: bool) -> usize {
        if let Some(found) = self.folders.get(&(parent, name.to_owned())) {
            return *found;
        }
        let node = self.push(parent, name, Kind::Folder { open });
        self.folders.insert((parent, name.to_owned()), node);
        self.folder_ids.push(node);
        node
    }

    fn push(&mut self, parent: Option<usize>, name: &str, kind: Kind) -> usize {
        let id = self.nodes.len();
        let index = match parent {
            Some(parent) => self.nodes[parent].children.len(),
            None => self.roots.len(),
        };
        self.nodes.push(Node {
            name: name.to_owned(),
            parent,
            index,
            children: Vec::new(),
            kind,
            counts: [0; STATES],
            tests: 0,
        });
        match parent {
            Some(parent) => self.nodes[parent].children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    /// One more test in state `state` in the folder and every folder above it. Only folders
    /// count: a test is in the state its row says.
    fn count(&mut self, folder: Option<usize>, state: usize) {
        let mut at = folder;
        while let Some(id) = at {
            let node = &mut self.nodes[id];
            node.counts[state] += 1;
            node.tests += 1;
            at = node.parent;
        }
    }

    /// The test in row `row` is now in state `to` and was in `from`.
    pub(crate) fn moved(&mut self, row: usize, from: RowStatus, to: RowStatus) {
        let mut at = self.nodes[self.test_nodes[row]].parent;
        while let Some(id) = at {
            let node = &mut self.nodes[id];
            node.counts[from.index()] -= 1;
            node.counts[to.index()] += 1;
            at = node.parent;
        }
    }

    /// Counts everything again, from the state of every test in the order of the rows: for when
    /// most of the tests change at once, which one by one would walk the folders above each.
    pub(crate) fn recount(&mut self, states: impl Iterator<Item = RowStatus>) {
        for id in &self.folder_ids {
            self.nodes[*id].counts = [0; STATES];
        }
        for (row, state) in states.enumerate() {
            if let Some(folder) = self.nodes[self.test_nodes[row]].parent {
                self.nodes[folder].counts[state.index()] += 1;
            }
        }
        // A folder is made before what is in it, so going backwards adds up from the bottom.
        for id in self.folder_ids.iter().rev() {
            let Some(parent) = self.nodes[*id].parent else {
                continue;
            };
            let counts = self.nodes[*id].counts;
            for (state, count) in counts.iter().enumerate() {
                self.nodes[parent].counts[state] += count;
            }
        }
    }

    /// Opens or closes a folder (the other kind of node is left as it is).
    pub(crate) fn set_open(&mut self, node: usize, to: bool) {
        if let Kind::Folder { open } = &mut self.nodes[node].kind {
            *open = to;
        }
    }

    pub(crate) fn is_open(&self, node: usize) -> bool {
        matches!(self.nodes[node].kind, Kind::Folder { open: true })
    }

    /// How deep the node is: 0 at the top.
    pub(crate) fn depth(&self, node: usize) -> usize {
        let mut depth = 0;
        let mut at = self.nodes[node].parent;
        while let Some(id) = at {
            depth += 1;
            at = self.nodes[id].parent;
        }
        depth
    }

    /// The node after this one as the tree is shown: its first child when it is an open folder,
    /// else the next at its level, else the next of the folder above.
    pub(crate) fn next(&self, node: usize) -> Option<usize> {
        let this = &self.nodes[node];
        if self.is_open(node)
            && let Some(first) = this.children.first()
        {
            return Some(*first);
        }
        let mut at = node;
        loop {
            let next = self.sibling(at, 1);
            if next.is_some() {
                return next;
            }
            at = self.nodes[at].parent?;
        }
    }

    /// The node before this one as the tree is shown: the last of what is open in the one before
    /// it at its level, else the folder it is in.
    pub(crate) fn previous(&self, node: usize) -> Option<usize> {
        let Some(before) = self.sibling(node, -1) else {
            return self.nodes[node].parent;
        };
        let mut last = before;
        while self.is_open(last)
            && let Some(child) = self.nodes[last].children.last()
        {
            last = *child;
        }
        Some(last)
    }

    /// The node `by` places along the nodes at its level.
    fn sibling(&self, node: usize, by: isize) -> Option<usize> {
        let siblings = match self.nodes[node].parent {
            Some(parent) => &self.nodes[parent].children,
            None => &self.roots,
        };
        siblings
            .get(self.nodes[node].index.checked_add_signed(by)?)
            .copied()
    }

    /// The rows of every test in the node.
    pub(crate) fn rows_under(&self, node: usize) -> Vec<usize> {
        let mut rows = Vec::with_capacity(self.nodes[node].tests);
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            match self.nodes[id].kind {
                Kind::Test { row } => rows.push(row),
                Kind::Folder { .. } => stack.extend(self.nodes[id].children.iter().rev()),
            }
        }
        rows
    }
}
