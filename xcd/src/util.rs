// Copyright (c) 2023 Yan Ka, Chiu.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions, and the following disclaimer,
//    without modification, immediately at the beginning of the file.
// 2. The name of the author may not be used to endorse or promote products
//    derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
// ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR
// ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
// DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
// OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
// HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
// LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
// OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
// SUCH DAMAGE.
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

pub struct TwoWayMap<K, V> {
    reverse_map: HashMap<V, HashSet<K>>,
    main_map: HashMap<K, V>,
}

impl<K, V> Default for TwoWayMap<K, V> {
    fn default() -> TwoWayMap<K, V> {
        TwoWayMap {
            reverse_map: HashMap::new(),
            main_map: HashMap::new(),
        }
    }
}

impl<K: Clone + Hash + Eq, V: Clone + Hash + Eq> TwoWayMap<K, V> {
    pub fn new() -> TwoWayMap<K, V> {
        TwoWayMap {
            reverse_map: HashMap::new(),
            main_map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.main_map.insert(key.clone(), value.clone());
        match self.reverse_map.get_mut(&value) {
            Some(vec) => {
                vec.insert(key);
            }
            None => {
                self.reverse_map
                    .insert(value.clone(), HashSet::from_iter([key]));
            }
        }
    }

    pub fn get<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.main_map.get(key)
    }

    #[allow(unused)]
    pub fn contains_key<Q: Eq + Hash + ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        self.main_map.get(key)
    }

    pub fn contains_value<Q: Eq + Hash + ?Sized>(&self, value: &Q) -> bool
    where
        V: Borrow<Q>,
    {
        self.reverse_map.contains_key(value)
    }

    /// Remove all keys that referenced the value
    pub fn remove_all_referenced<Q>(&mut self, value: &Q) -> Option<HashSet<K>>
    where
        V: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let keys = self.reverse_map.remove(value)?;
        for key in keys.iter() {
            self.main_map.remove(key);
        }
        Some(keys)
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let value = self.main_map.remove(key);
        if let Some(value) = &value {
            if let Some(keys) = self.reverse_map.get_mut(value) {
                keys.remove(key);
                if keys.is_empty() {
                    self.reverse_map.remove(value);
                }
            }
        }
        value
    }
}
