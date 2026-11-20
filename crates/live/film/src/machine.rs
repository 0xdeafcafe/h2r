//! The engine's private bootstrap, machine keyframes and between-keyframe
//! continuation data form one logical section. Its runs may be physically
//! separated by independently fetchable chronological TICK frames.
use crate::{Entry, KEYF, MAX_CHUNK, MAX_ENTRIES, Reader, read_bytes};
use anyhow::{Context, Result, ensure};
use live_proto::film::machine_index::{MachineRuns, is_machine};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{Error, SeqAccess, Visitor},
    ser::SerializeSeq,
};
use std::{
    fmt,
    io::{Read, Seek},
};

#[derive(Debug, Clone, Copy)]
pub struct MachineSection<'a> {
    entries: &'a [Entry],
    unified: bool,
}

impl<R: Read + Seek> Reader<R> {
    /// Private bootstrap, device/HLE results, controls, keyframes and cursor
    /// metadata, in capture order. Legacy films use their original flat index.
    pub fn machine_section(&self) -> MachineSection<'_> {
        MachineSection {
            entries: &self.entries,
            unified: self.header.version == 3,
        }
    }
}

impl<'a> MachineSection<'a> {
    /// Maximal contiguous runs in file order. Each row addresses an independent
    /// zstd frame. No TICK payload or public directory needs to be inflated.
    pub fn runs(self) -> impl DoubleEndedIterator<Item = &'a [Entry]> {
        self.entries
            .chunk_by(move |a, b| !self.unified || is_machine(a.tag) == is_machine(b.tag))
            .filter(move |run| !self.unified || is_machine(run[0].tag))
    }

    pub fn entries(self) -> impl DoubleEndedIterator<Item = &'a Entry> {
        self.runs().flatten()
    }

    /// Resolve a mandatory single record, checking both omissions and duplicates.
    pub fn unique(self, tag: crate::Tag) -> Result<&'a Entry> {
        let mut rows = self.entries().filter(|e| e.tag == tag);
        let name = String::from_utf8_lossy(&tag);
        let row = rows
            .next()
            .with_context(|| format!("missing film section {name}"))?;
        ensure!(rows.next().is_none(), "duplicate film section {name}");
        Ok(row)
    }

    /// Verify a private seek cursor before dropping consumed continuation calls.
    pub fn keyframe_at(self, offset: u64) -> Result<&'a Entry> {
        let index = self
            .entries
            .binary_search_by_key(&offset, |entry| entry.offset)
            .ok()
            .context("keyframe offset is absent from the machine section")?;
        let entry = &self.entries[index];
        ensure!(entry.tag == KEYF, "machine cursor is not a keyframe");
        Ok(entry)
    }
}

#[derive(Serialize, Deserialize)]
struct Row {
    tag: crate::Tag,
    offset: u64,
    size: u32,
    /// Present only on the first row of a maximal machine run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine_span: Option<u32>,
}

/// Keep the physical row sequence streamable by Live. Public row counts,
/// offsets and lengths remain implicit in the existing row data.
pub(super) fn encode(entries: &[Entry]) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut serializer = serde_json::Serializer::new(&mut raw);
    let mut seq = serializer.serialize_seq(Some(entries.len()))?;
    for group in entries.chunk_by(|a, b| is_machine(a.tag) == is_machine(b.tag)) {
        let mut span = if is_machine(group[0].tag) {
            Some(u32::try_from(group.len())?)
        } else {
            None
        };
        for e in group {
            seq.serialize_element(&Row {
                tag: e.tag,
                offset: e.offset,
                size: e.size,
                machine_span: span.take(),
            })?;
        }
    }
    seq.end()?;
    Ok(raw)
}

struct Rows;
impl<'de> Visitor<'de> for Rows {
    type Value = Vec<Entry>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a film machine section index")
    }
    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut entries = Vec::new();
        let mut runs = MachineRuns::default();
        while let Some(row) = seq.next_element::<Row>()? {
            if entries.len() >= MAX_ENTRIES {
                return Err(A::Error::custom("too many film envelopes"));
            }
            runs.feed(row.tag, row.machine_span)
                .map_err(A::Error::custom)?;
            entries.push(Entry {
                tag: row.tag,
                offset: row.offset,
                size: row.size,
            });
        }
        runs.finish().map_err(A::Error::custom)?;
        Ok(entries)
    }
}

pub(super) fn decode<R: Read + Seek>(input: &mut R, index: &Entry) -> Result<Vec<Entry>> {
    let raw = read_bytes(input, index, MAX_CHUNK)?;
    let mut d = serde_json::Deserializer::from_slice(&raw);
    let entries = d.deserialize_seq(Rows)?;
    d.end()?;
    Ok(entries)
}

#[cfg(test)]
mod tests;
