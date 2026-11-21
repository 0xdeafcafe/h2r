//! Bound sequence and map decoding before admitting a definition.
//! The sequence visitor uses the same first-excess-element check as Forge.

use std::{collections::BTreeMap, fmt, io, marker::PhantomData};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};

use crate::{Refusal, Result};

pub(crate) fn check_size(len: usize, field: &'static str, max: usize) -> Result<()> {
    if len > max {
        return Err(Refusal::Limit { field, max });
    }
    Ok(())
}

pub(crate) fn json_error(error: serde_json::Error) -> Refusal {
    Refusal::Json(error.to_string().chars().take(256).collect())
}

pub(crate) fn bounded_seq<'de, D, T, const N: usize>(
    decoder: D,
) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const N: usize>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for Bounded<T, N> {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an array with at most {N} items")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<T>, A::Error> {
            let mut items = Vec::new();
            while items.len() < N {
                let Some(item) = seq.next_element()? else {
                    return Ok(items);
                };
                items.push(item);
            }
            seq.next_element_seed(Excess(&self))?;
            Ok(items)
        }
    }

    struct Excess<'a, T, const N: usize>(&'a Bounded<T, N>);

    impl<'de, T: Deserialize<'de>, const N: usize> DeserializeSeed<'de> for Excess<'_, T, N> {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, _: D) -> std::result::Result<(), D::Error> {
            Err(de::Error::invalid_length(N + 1, self.0))
        }
    }

    decoder.deserialize_seq(Bounded::<T, N>(PhantomData))
}

pub(crate) fn bounded_map<'de, D, K, V, const N: usize>(
    decoder: D,
) -> std::result::Result<BTreeMap<K, V>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de> + Ord,
    V: Deserialize<'de>,
{
    struct Bounded<K, V, const N: usize>(PhantomData<(K, V)>);

    impl<'de, K, V, const N: usize> Visitor<'de> for Bounded<K, V, N>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        type Value = BTreeMap<K, V>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an object with at most {N} entries")
        }

        fn visit_map<A: MapAccess<'de>>(
            self,
            mut map: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            use std::collections::btree_map::Entry;

            let mut items = BTreeMap::new();
            while let Some(key) = map.next_key()? {
                if items.len() == N {
                    return Err(de::Error::invalid_length(N + 1, &self));
                }
                let Entry::Vacant(entry) = items.entry(key) else {
                    return Err(de::Error::custom("duplicate scoring key"));
                };
                entry.insert(map.next_value()?);
            }
            Ok(items)
        }
    }

    decoder.deserialize_map(Bounded::<K, V, N>(PhantomData))
}

/// On this integer schema the compact serialization has the same size as
/// canonical JSON. Count the bytes without building an intermediate tree.
pub(crate) fn measure(value: &impl Serialize, max: usize) -> Result<()> {
    struct Size(usize);

    impl io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or_else(|| io::Error::other("size limit"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    serde_json::to_writer(&mut Size(max), value).map_err(|error| {
        if error.is_io() {
            Refusal::Limit {
                field: "document bytes",
                max,
            }
        } else {
            json_error(error)
        }
    })
}
