//! Data-snapshot values decoded on first use. The template cell and entity
//! lists are most of the snapshot but a cluster uses few templates, so the
//! snapshot keeps each one as its own MessagePack blob and decodes it when
//! a placement first reads it.
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};
use std::sync::OnceLock;

pub struct Lazy<T> { encoded: Vec<u8>, value: OnceLock<T> }

impl<T> Lazy<T> {
    /// An already decoded value (the YAML loader's).
    pub fn new(value: T) -> Self { Lazy { encoded: Vec::new(), value: OnceLock::from(value) } }
}

impl<T: DeserializeOwned> Lazy<T> {
    pub fn get(&self) -> &T {
        self.value.get_or_init(|| rmp_serde::from_slice(&self.encoded).expect("data snapshot value decodes"))
    }
}

impl<T: Serialize + DeserializeOwned> Serialize for Lazy<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = rmp_serde::to_vec(self.get()).map_err(serde::ser::Error::custom)?;
        serializer.serialize_bytes(&encoded)
    }
}

impl<'de, T> Deserialize<'de> for Lazy<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl<'de> serde::de::Visitor<'de> for Bytes {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("an encoded snapshot value") }
            fn visit_bytes<E>(self, v: &[u8]) -> Result<Vec<u8>, E> { Ok(v.to_vec()) }
            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Vec<u8>, E> { Ok(v) }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
                let mut v = Vec::new();
                while let Some(b) = seq.next_element()? { v.push(b); }
                Ok(v)
            }
        }
        Ok(Lazy { encoded: deserializer.deserialize_byte_buf(Bytes)?, value: OnceLock::new() })
    }
}

impl<T: Clone> Clone for Lazy<T> {
    fn clone(&self) -> Self { Lazy { encoded: self.encoded.clone(), value: self.value.clone() } }
}

impl<T: std::fmt::Debug + DeserializeOwned> std::fmt::Debug for Lazy<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.get().fmt(f) }
}

impl<T: DeserializeOwned> Lazy<T> {
    pub fn get_mut(&mut self) -> &mut T {
        self.get();
        self.value.get_mut().expect("decoded")
    }
    pub fn into_inner(self) -> T {
        self.get();
        self.value.into_inner().expect("decoded")
    }
}

/// A `BTreeMap` whose values are [`Lazy`]: the map API, returning decoded
/// values, for the large data-snapshot maps.
pub struct LazyMap<K, V>(std::collections::BTreeMap<K, Lazy<V>>);

impl<K, V> Default for LazyMap<K, V> {
    fn default() -> Self { LazyMap(Default::default()) }
}

impl<K: Clone, V: Clone> Clone for LazyMap<K, V> {
    fn clone(&self) -> Self { LazyMap(self.0.clone()) }
}

impl<K: Ord, V: DeserializeOwned> LazyMap<K, V> {
    pub fn get<Q: Ord + ?Sized>(&self, key: &Q) -> Option<&V> where K: std::borrow::Borrow<Q> { self.0.get(key).map(Lazy::get) }
    pub fn get_mut<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<&mut V> where K: std::borrow::Borrow<Q> { self.0.get_mut(key).map(Lazy::get_mut) }
    pub fn contains_key<Q: Ord + ?Sized>(&self, key: &Q) -> bool where K: std::borrow::Borrow<Q> { self.0.contains_key(key) }
    pub fn insert(&mut self, key: K, value: V) -> Option<V> { self.0.insert(key, Lazy::new(value)).map(Lazy::into_inner) }
    pub fn remove<Q: Ord + ?Sized>(&mut self, key: &Q) -> Option<V> where K: std::borrow::Borrow<Q> { self.0.remove(key).map(Lazy::into_inner) }
    pub fn len(&self) -> usize { self.0.len() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
    pub fn keys(&self) -> std::collections::btree_map::Keys<'_, K, Lazy<V>> { self.0.keys() }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + '_ { self.0.values().map(Lazy::get) }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + '_ { self.0.iter().map(|(k, v)| (k, v.get())) }
}

impl<'a, K: Ord, V: DeserializeOwned> IntoIterator for &'a LazyMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::iter::Map<std::collections::btree_map::Iter<'a, K, Lazy<V>>, fn((&'a K, &'a Lazy<V>)) -> (&'a K, &'a V)>;
    fn into_iter(self) -> Self::IntoIter { self.0.iter().map(|(k, v)| (k, v.get())) }
}

impl<K: Ord + std::borrow::Borrow<Q>, Q: Ord + ?Sized, V: DeserializeOwned> std::ops::Index<&Q> for LazyMap<K, V> {
    type Output = V;
    fn index(&self, key: &Q) -> &V { self.get(key).expect("key in map") }
}

impl<K: Ord, V> FromIterator<(K, V)> for LazyMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self { LazyMap(iter.into_iter().map(|(k, v)| (k, Lazy::new(v))).collect()) }
}

impl<K: Serialize, V: Serialize + DeserializeOwned> Serialize for LazyMap<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> { self.0.serialize(serializer) }
}

impl<'de, K: Deserialize<'de> + Ord, V> Deserialize<'de> for LazyMap<K, V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(LazyMap(std::collections::BTreeMap::deserialize(deserializer)?))
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug + DeserializeOwned> std::fmt::Debug for LazyMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_map().entries(self.0.iter().map(|(k, v)| (k, v.get()))).finish() }
}
