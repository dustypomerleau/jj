// Copyright 2024 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(missing_docs)]

use std::any::{Any, TypeId};

use once_map::sync::OnceMap;

/// Type-safe, thread-safe map that stores objects of arbitrary types.
///
/// This allows extensions to store and retrieve their own types unknown to
/// jj_lib safely.
#[derive(Default)]
pub struct AnyMap {
    values: OnceMap<TypeId, Box<dyn Any>>,
}

impl AnyMap {
    /// Creates an empty AnyMap.
    pub fn new() -> Self {
        Self {
            values: OnceMap::new(),
        }
    }

    /// Returns the specified type if it has already been inserted.
    pub fn get<V: Any>(&self) -> Option<&V> {
        self.values
            .get(&TypeId::of::<V>())
            .map(|v| v.downcast_ref::<V>().unwrap())
    }

    /// Inserts a new instance of the specified type if it doesn't already
    /// exist.
    pub fn insert<V: Any>(&self, generator: impl FnOnce() -> V) -> &V {
        self.values
            .insert(TypeId::of::<V>(), move |_| Box::new(generator()))
            .downcast_ref::<V>()
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTypeA;
    impl TestTypeA {
        fn get_a(&self) -> &'static str {
            "a"
        }
    }

    struct TestTypeB;
    impl TestTypeB {
        fn get_b(&self) -> &'static str {
            "b"
        }
    }

    #[test]
    fn test_empty() {
        let any_map = AnyMap::new();
        assert!(any_map.get::<TestTypeA>().is_none());
        assert!(any_map.get::<TestTypeB>().is_none());
    }

    #[test]
    fn test_retrieval() {
        let any_map = AnyMap::new();
        assert_eq!(any_map.insert::<TestTypeA>(|| TestTypeA).get_a(), "a");
        assert_eq!(any_map.insert::<TestTypeB>(|| TestTypeB).get_b(), "b");
        assert_eq!(
            any_map.get::<TestTypeA>().map(|a| a.get_a()).unwrap_or(""),
            "a"
        );
        assert_eq!(
            any_map.get::<TestTypeB>().map(|b| b.get_b()).unwrap_or(""),
            "b"
        );
    }
}
