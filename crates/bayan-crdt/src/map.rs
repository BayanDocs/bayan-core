//! Property maps, movable identifier lists, and the root registries that hold them.
//!
//! Every handle shares the poison flag of its document: once the document is poisoned (see [`crate::CrdtError::Poisoned`]), every method refuses or answers as for an empty map, list or registry, without calling the CRDT library.

use std::collections::BTreeMap;

use loro::{LoroMap, LoroMovableList, ValueOrContainer};

use crate::poison::Poison;
use crate::{CrdtError, Story, Value};

/// A map from property names to values, such as the properties of one paragraph, table, row or cell. Concurrent writes to the same key resolve deterministically (the last writer, in the CRDT's order, wins); writes to different keys never conflict.
///
/// Cloning a `PropertyMap` gives another handle to the same map.
#[derive(Debug, Clone)]
pub struct PropertyMap {
    map: LoroMap,
    poison: Poison,
}

impl PropertyMap {
    pub(crate) const fn new(map: LoroMap, poison: Poison) -> Self {
        Self { map, poison }
    }

    /// The value of `key`, if it is set to a value (a nested list or map is not a value; see [`PropertyMap::id_list`]).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Value> {
        if self.poison.is_set() {
            return None;
        }
        match self.map.get(key)? {
            ValueOrContainer::Value(value) => Some(Value::from_loro(&value)),
            ValueOrContainer::Container(_) => None,
        }
    }

    /// Sets `key` to `value`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], [`CrdtError::InvalidValue`] for a value that cannot be stored, or [`CrdtError::Library`].
    pub fn set(&self, key: &str, value: &Value) -> Result<(), CrdtError> {
        self.poison.check()?;
        let value = value.to_loro()?;
        self.map.insert(key, value).map_err(CrdtError::library)
    }

    /// Removes `key`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`] or [`CrdtError::Library`].
    pub fn remove(&self, key: &str) -> Result<(), CrdtError> {
        self.poison.check()?;
        self.map.delete(key).map_err(CrdtError::library)
    }

    /// Every key that holds a value, in key order. Keys that hold nested containers (such as an identifier list) are left out.
    #[must_use]
    pub fn entries(&self) -> BTreeMap<String, Value> {
        let mut entries = BTreeMap::new();
        if self.poison.is_set() {
            return entries;
        }
        self.map.for_each(|key, item| {
            if let ValueOrContainer::Value(value) = item {
                entries.insert(key.to_owned(), Value::from_loro(&value));
            }
        });
        entries
    }

    /// The movable identifier list stored under `key`, created if there is none yet. Lists that several replicas create concurrently under the same key are the same list (its identity derives from the map and the key), so their items merge.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], or [`CrdtError::Library`] (for example when `key` holds a value).
    pub fn id_list(&self, key: &str) -> Result<IdList, CrdtError> {
        self.poison.check()?;
        self.map
            .ensure_mergeable_movable_list(key)
            .map(|list| IdList::new(list, self.poison.clone()))
            .map_err(CrdtError::library)
    }

    /// The movable identifier list stored under `key`, if there is one.
    #[must_use]
    pub fn get_id_list(&self, key: &str) -> Option<IdList> {
        if self.poison.is_set() {
            return None;
        }
        match self.map.get(key)? {
            ValueOrContainer::Container(container) => container
                .into_movable_list()
                .ok()
                .map(|list| IdList::new(list, self.poison.clone())),
            ValueOrContainer::Value(_) => None,
        }
    }
}

/// An ordered list of identifiers (table rows, row cells) whose items can be inserted, deleted and moved concurrently: a concurrent move and delete of the same item never duplicates it, and two concurrent moves leave it in exactly one place.
///
/// Cloning an `IdList` gives another handle to the same list.
#[derive(Debug, Clone)]
pub struct IdList {
    list: LoroMovableList,
    poison: Poison,
}

impl IdList {
    const fn new(list: LoroMovableList, poison: Poison) -> Self {
        Self { list, poison }
    }

    /// The number of items (0 once the document is poisoned).
    #[must_use]
    pub fn len(&self) -> usize {
        if self.poison.is_set() {
            return 0;
        }
        self.list.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The items in order. Items written by a well-behaved replica are strings; anything else that another replica wrote is returned as it is, for the model to reject.
    #[must_use]
    pub fn items(&self) -> Vec<Value> {
        let len = self.len();
        let mut items = Vec::with_capacity(len);
        for index in 0..len {
            items.push(match self.list.get(index) {
                Some(ValueOrContainer::Value(value)) => Value::from_loro(&value),
                Some(ValueOrContainer::Container(_)) | None => Value::Unsupported,
            });
        }
        items
    }

    /// Inserts `id` before position `index` (`index == len()` appends).
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], [`CrdtError::OutOfRange`] or [`CrdtError::Library`].
    pub fn insert(&self, index: usize, id: &str) -> Result<(), CrdtError> {
        self.poison.check()?;
        let len = self.len();
        if index > len {
            return Err(CrdtError::OutOfRange { pos: index, len });
        }
        self.list.insert(index, id).map_err(CrdtError::library)
    }

    /// Deletes the item at `index`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], [`CrdtError::OutOfRange`] or [`CrdtError::Library`].
    pub fn delete(&self, index: usize) -> Result<(), CrdtError> {
        self.poison.check()?;
        let len = self.len();
        if index >= len {
            return Err(CrdtError::OutOfRange { pos: index, len });
        }
        self.list.delete(index, 1).map_err(CrdtError::library)
    }

    /// Moves the item at `from` so that it ends up at position `to`.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`], [`CrdtError::OutOfRange`] or [`CrdtError::Library`].
    pub fn move_item(&self, from: usize, to: usize) -> Result<(), CrdtError> {
        self.poison.check()?;
        let len = self.len();
        if from >= len || to >= len {
            return Err(CrdtError::OutOfRange {
                pos: from.max(to),
                len,
            });
        }
        if from == to {
            return Ok(());
        }
        self.list.mov(from, to).map_err(CrdtError::library)
    }
}

/// A root map from entity identifiers to property maps, for one kind of entity (paragraphs, tables, rows, cells, objects, fields, comments, ranges).
#[derive(Debug, Clone)]
pub struct Registry {
    map: LoroMap,
    poison: Poison,
}

impl Registry {
    pub(crate) const fn new(map: LoroMap, poison: Poison) -> Self {
        Self { map, poison }
    }

    /// The property map of entity `id`, if it exists.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<PropertyMap> {
        if self.poison.is_set() {
            return None;
        }
        match self.map.get(id)? {
            ValueOrContainer::Container(container) => container
                .into_map()
                .ok()
                .map(|map| PropertyMap::new(map, self.poison.clone())),
            ValueOrContainer::Value(_) => None,
        }
    }

    /// The property map of entity `id`, created empty if it does not exist. Maps that several replicas create concurrently for the same identifier are the same map (its identity derives from the registry and the identifier), so their properties merge key by key. Entity identifiers are unique 128-bit numbers, so this happens only when replicas write a structure that normalization derived deterministically.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`] or [`CrdtError::Library`].
    pub fn create(&self, id: &str) -> Result<PropertyMap, CrdtError> {
        self.poison.check()?;
        self.map
            .ensure_mergeable_map(id)
            .map(|map| PropertyMap::new(map, self.poison.clone()))
            .map_err(CrdtError::library)
    }

    /// Every entity in the registry with its properties, in identifier order. Entries that are not property maps (written by a misbehaving replica) are left out.
    #[must_use]
    pub fn entries(&self) -> BTreeMap<String, PropertyMap> {
        let mut entries = BTreeMap::new();
        if self.poison.is_set() {
            return entries;
        }
        self.map.for_each(|key, item| {
            if let ValueOrContainer::Container(container) = item
                && let Ok(map) = container.into_map()
            {
                entries.insert(key.to_owned(), PropertyMap::new(map, self.poison.clone()));
            }
        });
        entries
    }

    /// The identifiers of every entity, in identifier order. Much cheaper than [`Registry::entries`]: the library does not have to decode each entry's container reference.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        if self.poison.is_set() {
            return Vec::new();
        }
        let mut ids: Vec<String> = self.map.keys().map(|key| key.to_string()).collect();
        ids.sort_unstable();
        ids
    }

    /// The number of entries (0 once the document is poisoned).
    #[must_use]
    pub fn len(&self) -> usize {
        if self.poison.is_set() {
            return 0;
        }
        self.map.len()
    }

    /// Whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The root map from story identifiers to stories, for every story except the main one (which is a root sequence of its own, see [`crate::Doc::main_story`]).
#[derive(Debug, Clone)]
pub struct Stories {
    map: LoroMap,
    poison: Poison,
}

impl Stories {
    pub(crate) const fn new(map: LoroMap, poison: Poison) -> Self {
        Self { map, poison }
    }

    /// The story `id`, if it exists.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Story> {
        if self.poison.is_set() {
            return None;
        }
        match self.map.get(id)? {
            ValueOrContainer::Container(container) => container
                .into_text()
                .ok()
                .map(|text| Story::new(text, self.poison.clone())),
            ValueOrContainer::Value(_) => None,
        }
    }

    /// The story `id`, created empty if it does not exist. Stories that several replicas create concurrently with the same identifier are the same story, so their atoms merge.
    ///
    /// # Errors
    ///
    /// [`CrdtError::Poisoned`] or [`CrdtError::Library`].
    pub fn create(&self, id: &str) -> Result<Story, CrdtError> {
        self.poison.check()?;
        self.map
            .ensure_mergeable_text(id)
            .map(|text| Story::new(text, self.poison.clone()))
            .map_err(CrdtError::library)
    }

    /// The identifiers of every story, in identifier order (cheap, as for [`Registry::ids`]).
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        if self.poison.is_set() {
            return Vec::new();
        }
        let mut ids: Vec<String> = self.map.keys().map(|key| key.to_string()).collect();
        ids.sort_unstable();
        ids
    }

    /// Every story, in identifier order.
    #[must_use]
    pub fn entries(&self) -> BTreeMap<String, Story> {
        let mut entries = BTreeMap::new();
        if self.poison.is_set() {
            return entries;
        }
        self.map.for_each(|key, item| {
            if let ValueOrContainer::Container(container) = item
                && let Ok(text) = container.into_text()
            {
                entries.insert(key.to_owned(), Story::new(text, self.poison.clone()));
            }
        });
        entries
    }
}
