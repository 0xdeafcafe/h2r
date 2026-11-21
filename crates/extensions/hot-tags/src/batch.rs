use super::*;
use std::collections::BTreeMap;

#[derive(Debug)]
pub enum BatchError {
    Field(Error),
    Mismatch(Vec<Value>),
}

impl From<Error> for BatchError {
    fn from(error: Error) -> Self {
        Self::Field(error)
    }
}

impl Editor {
    /// Resolve and compare the entire request while the guest is locked. Check write
    /// permissions before the first store, including pages crossed by a field.
    pub fn batch(
        &mut self,
        g: &mut Guest,
        writes: &[Value],
        label: Option<&str>,
    ) -> std::result::Result<Value, BatchError> {
        let idx = self.index(g)?;
        let mut entries = None;
        let mut fields = Vec::new();
        let mut changes = Vec::new();
        let mut ranges = BTreeMap::<u32, u64>::new();
        for w in writes {
            let v = w
                .get("value")
                .ok_or_else(|| Error::invalid("missing value"))?;
            let (entry, addr, kind, path) = if let Some(at) = w.get("at") {
                if w.get("tag").is_some() || w.get("path").is_some() {
                    return Err(Error::invalid("give at or tag and path").into());
                }
                let addr = at
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .or_else(|| u32::from_str_radix(at.as_str()?.strip_prefix("0x")?, 16).ok())
                    .ok_or_else(|| Error::invalid("at must be a 32-bit address"))?;
                let s = v
                    .as_str()
                    .filter(|s| {
                        !s.is_empty()
                            && s.len() % 2 == 0
                            && s.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                    .ok_or_else(|| Error::invalid("value must be nonempty hex bytes"))?;
                let len =
                    u32::try_from(s.len() / 2).map_err(|_| Error::invalid("value too large"))?;
                if entries.is_none() {
                    entries = Some(index::all(g, &idx).map_err(Error::unavailable)?);
                }
                let end = u64::from(addr) + u64::from(len);
                let e = entries
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|e| {
                        e.address <= addr
                            && end <= u64::from(e.address) + u64::from(e.size)
                            && end <= 1u64 << 32
                    })
                    .ok_or_else(|| {
                        Error::invalid(format!("{addr:#010x}..{end:#x} outside loaded tags"))
                    })?;
                (
                    e.clone(),
                    addr,
                    tags::defs::Kind::Unknown {
                        len,
                        seen: String::new(),
                    },
                    format!("+{:#x}:bytes[{len}]", addr - e.address),
                )
            } else {
                let tag = w["tag"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("missing tag"))?;
                let path = w["path"]
                    .as_str()
                    .ok_or_else(|| Error::invalid("missing path"))?;
                let sel = TagSel {
                    tag: tag.into(),
                    group: w["group"].as_str().map(str::to_owned),
                };
                let (_, e, loc) = self.locate(g, &sel, path)?;
                (e, loc.addr, loc.kind, path.into())
            };
            if !value::writable(&kind) {
                return Err(Error::invalid(format!("{path} is structural")).into());
            }
            let len = value::width(&kind);
            let end = u64::from(addr) + u64::from(len);
            if ranges
                .range(..=addr)
                .next_back()
                .is_some_and(|(_, last)| *last > u64::from(addr))
                || ranges
                    .range(addr..)
                    .next()
                    .is_some_and(|(next, _)| u64::from(*next) < end)
            {
                return Err(Error::invalid("overlapping fields in edit.batch").into());
            }
            ranges.insert(addr, end);
            g.check(addr, len as usize, runtime::guest::Access::Write, 1)
                .map_err(fault)?;
            let current = read_bytes(g, addr, len)?;
            let group_of = |d| index::by_datum(g, &idx, d).ok().map(|e| e.group);
            let encode = |v| value::encode(&kind, v, &current, &group_of).map_err(Error::invalid);
            let bytes = encode(v)?;
            if let Some(was) = w.get("was") {
                let expected = encode(was)?;
                if current != expected && current != bytes {
                    fields.push(json!({"at":addr,"tag":format!("{:#010x}",entry.datum),"path":path,
                        "was":was,"holds":value::read(&kind,&current),"holds_bytes":value::hex(&current)}));
                }
            }
            changes.push(Change {
                id: self.next_id + 1,
                scenario: idx.scenario,
                tag: entry.datum,
                group: group_text(entry.group),
                name: self
                    .names(g, &idx)
                    .and_then(|n| n.name(entry.datum))
                    .map(str::to_owned),
                path,
                label: label.map(str::to_owned),
                addr,
                kind: value::kind_text(&kind),
                old: value::read(&kind, &current),
                new: value::read(&kind, &bytes),
                old_bytes: current,
                new_bytes: bytes,
                kind_def: kind,
                batch: true,
            });
        }
        if !fields.is_empty() {
            return Err(BatchError::Mismatch(fields));
        }
        self.next_id += 1;
        for c in &changes {
            g.write_bytes(c.addr, &c.new_bytes, 1).map_err(fault)?;
        }
        let reply = json!({"id":self.next_id,"applied":changes.len(),"changes":changes});
        self.journal
            .extend(changes.into_iter().filter(|c| c.old_bytes != c.new_bytes));
        Ok(reply)
    }
}
