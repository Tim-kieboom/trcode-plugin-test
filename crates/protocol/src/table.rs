//! A list of tests kept in one piece of text.
//!
//! A plugin has a small budget for each message it handles, and freeing memory is not free:
//! a list of 3,000 tests as three `String`s each is 9,000 pieces to free the next time the
//! tests are asked for, which cost nearly twice a message's whole budget. Here the ids, names
//! and groups of all the tests are in one `String` and the rows are numbers into it, so a list
//! of any length is a few pieces to free, and it takes a tenth of the memory.
//!
//! Tests are numbered in the order they were added. An id is found by a hash of it kept in a
//! table of numbers (no key is a piece of memory of its own).

use std::collections::HashMap;

use crate::TestItem;

/// Where the parts of one test are in the text.
#[derive(Debug, Clone, Copy)]
struct Packed {
    group: u32,
    id: (u32, u32),
    name: (u32, u32),
}

#[derive(Debug, Default)]
pub struct Table {
    text: String,
    rows: Vec<Packed>,
    /// The groups, each once: most tests share the group of the one before.
    groups: Vec<String>,
    group_numbers: HashMap<String, u32>,
    /// The row of each id, by a hash of it. Two ids with the same hash are told apart by
    /// comparing them, and the second is put at the next hash.
    by_id: HashMap<u64, u32>,
}

impl Table {
    /// How many tests there are.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Forgets every test.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Adds a test; the number it has.
    pub fn push(&mut self, item: &TestItem) -> usize {
        self.push_parts(&item.id, &item.name, &item.group)
    }

    /// Adds a test made of these parts; the number it has.
    pub fn push_parts(&mut self, id: &str, name: &str, group: &str) -> usize {
        let row = self.rows.len();
        let packed = Packed {
            group: self.intern(group),
            id: self.append(id),
            name: self.append(name),
        };
        self.rows.push(packed);
        let mut key = hash(id);
        while self.by_id.contains_key(&key) {
            key = key.wrapping_add(1);
        }
        self.by_id.insert(key, row as u32);
        row
    }

    fn append(&mut self, part: &str) -> (u32, u32) {
        let start = self.text.len();
        self.text.push_str(part);
        (start as u32, part.len() as u32)
    }

    fn intern(&mut self, group: &str) -> u32 {
        if let Some(number) = self.group_numbers.get(group) {
            return *number;
        }
        let number = self.groups.len() as u32;
        self.groups.push(group.to_owned());
        self.group_numbers.insert(group.to_owned(), number);
        number
    }

    fn slice(&self, (start, len): (u32, u32)) -> &str {
        &self.text[start as usize..(start + len) as usize]
    }

    /// How many groups the tests are in.
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// The id of test `row`.
    pub fn id(&self, row: usize) -> &str {
        self.slice(self.rows[row].id)
    }

    /// The name of test `row`.
    pub fn name(&self, row: usize) -> &str {
        self.slice(self.rows[row].name)
    }

    /// The group of test `row`.
    pub fn group(&self, row: usize) -> &str {
        &self.groups[self.rows[row].group as usize]
    }

    /// Test `row` as it was added.
    pub fn item(&self, row: usize) -> TestItem {
        TestItem {
            id: self.id(row).to_owned(),
            name: self.name(row).to_owned(),
            group: self.group(row).to_owned(),
        }
    }

    /// The number of the test with this id.
    pub fn find(&self, id: &str) -> Option<usize> {
        let mut key = hash(id);
        loop {
            let row = *self.by_id.get(&key)? as usize;
            if self.id(row) == id {
                return Some(row);
            }
            key = key.wrapping_add(1);
        }
    }

    /// Up to `limit` tests from number `offset`.
    pub fn items(&self, offset: usize, limit: usize) -> Vec<TestItem> {
        let start = offset.min(self.len());
        let end = start.saturating_add(limit).min(self.len());
        (start..end).map(|row| self.item(row)).collect()
    }
}

/// FNV-1a, 64 bits: a few instructions for each byte, and no state to set up.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}
