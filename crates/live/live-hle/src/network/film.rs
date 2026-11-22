//! Capture the translated guest ABI, including WSAEMSGSIZE's successful
//! buffer writes. Replaying never enters the native socket implementation.
use super::*;
use ::film::tape::{Call, Tap};

impl Network {
    pub(super) fn filmed(
        &self,
        tap: &dyn Tap,
        op: Op,
        a: &[u32],
        thread: u32,
        rt: &Runtime,
    ) -> Result<u32> {
        let name = format!("{op:?}");
        if tap.replaying() {
            let call = tap.next(&name, a, thread).ok_or(10022u32)?;
            if let Err(e) = self.restore(op, a, rt, &call) {
                tap.fault(format!("cannot restore {name} call {} ({e})", call.index));
                return Err(e);
            }
            return call.result;
        }
        let result = self.run(op, a, thread, rt);
        match capture(op, a, rt, result) {
            Ok(outputs) => {
                let tag = if matches!(op, Op::Random | Op::CreateKey) {
                    ::film::RAND
                } else {
                    ::film::NETR
                };
                tap.record(
                    tag,
                    Call {
                        index: 0,
                        op: name,
                        args: a.to_vec(),
                        thread,
                        result,
                        outputs,
                    },
                );
            }
            Err(e) => tap.fault(format!("cannot record {name} outputs ({e})")),
        }
        result
    }

    fn restore(&self, op: Op, a: &[u32], rt: &Runtime, c: &Call) -> Result<()> {
        if op == Op::QosLookup && c.result.is_ok() {
            let [ptr, body] = c.outputs.as_slice() else {
                return Err(10022);
            };
            let addr = u32::from_le_bytes(ptr.as_slice().try_into().map_err(|_| 10022u32)?);
            if qos_size(body, a)? != body.len() {
                return Err(10022);
            }
            let count = a[0] + a[4];
            let mut offset = 8 + count * 24;
            for row in body[8..offset as usize].as_chunks::<24>().0 {
                let size = u32::from(u16::from_le_bytes([row[6], row[7]]));
                let ptr = u32::from_le_bytes(row[8..12].try_into().unwrap());
                let expected = if size == 0 {
                    0
                } else {
                    addr.checked_add(offset).ok_or(10022u32)?
                };
                if ptr != expected {
                    return Err(10022);
                }
                offset += size;
            }
            let mut m = rt.memory();
            let allocated = lock(&self.state)
                .qos
                .alloc(&mut m, body.len() as u32)
                .ok_or(10055u32)?;
            if allocated != addr {
                return Err(10022);
            }
            m.write_bytes(allocated, body, 1).map_err(|_| 10014u32)?;
            m.set_u32(a[11], allocated).map_err(|_| 10014u32)?;
            return Ok(());
        }
        // Registered keys are also consumed by the local session and stats HLE.
        if matches!(op, Op::RegisterKey | Op::UnregisterKey) && c.result.is_ok() {
            self.register_key(op, a, rt)?;
        }
        let slots = slots(op, a, c.result)?;
        if slots.len() != c.outputs.len() {
            return Err(10022);
        }
        for ((p, size), bytes) in slots.into_iter().zip(&c.outputs) {
            if op == Op::Select {
                let n = word(rt, p)?;
                if n > 64 || set_size(bytes)? != bytes.len() || bytes.len() > 4 + n as usize * 4 {
                    return Err(10022);
                }
            } else if bytes.len() != size {
                return Err(10022);
            }
            write(rt, p, bytes)?;
        }
        Ok(())
    }
}

/// Known guest output pointers. The film cannot choose write addresses.
fn slots(op: Op, a: &[u32], result: Result<u32>) -> Result<Vec<(u32, usize)>> {
    use Op::*;
    let n = match result {
        Ok(n) => n,
        Err(10040) if matches!(op, Recv | RecvFrom) => a[2],
        Err(_) => return Ok(vec![]),
    };
    let mut slots = vec![];
    match op {
        Recv | RecvFrom => {
            if n > a[2] || n > 16 << 20 {
                return Err(10022);
            }
            slots.push((a[1], n as usize));
            if op == RecvFrom && a[4] != 0 {
                slots.extend([(a[4], 16), (a[5], 4)]);
            }
        }
        Select => slots.extend(a[1..4].iter().filter(|p| **p != 0).map(|p| (*p, 0))),
        Ioctl if a[1] == 0x4004667f => slots.push((a[2], 4)),
        GetOption => slots.extend([(a[3], 4), (a[4], 4)]),
        Random => {
            if a[1] > 1 << 20 {
                return Err(10022);
            }
            slots.push((a[0], a[1] as usize));
        }
        CreateKey => slots.extend([(a[0], 8), (a[1], 16)]),
        XnToIn => slots.push((a[2], 4)),
        InToXn => {
            slots.push((a[1], 36));
            if a[2] != 0 {
                slots.push((a[2], 8));
            }
        }
        _ => {}
    }
    Ok(slots)
}
fn set_size(prefix: &[u8]) -> Result<usize> {
    let n = u32::from_le_bytes(prefix.get(..4).ok_or(10022u32)?.try_into().unwrap());
    if n > 64 {
        return Err(10022);
    }
    Ok(4 + n as usize * 4)
}
fn qos_size(table: &[u8], a: &[u32]) -> Result<usize> {
    let n = a[0]
        .checked_add(a[4])
        .filter(|&n| n <= 64)
        .ok_or(10022u32)?;
    if table.get(..4) != Some(n.to_le_bytes().as_slice()) {
        return Err(10022);
    }
    let end = 8 + n as usize * 24;
    let mut size = end;
    for row in table.get(8..end).ok_or(10022u32)?.as_chunks::<24>().0 {
        let bytes = u16::from_le_bytes([row[6], row[7]]) as usize;
        if bytes > 1020 {
            return Err(10022);
        }
        size += bytes;
    }
    Ok(size)
}
fn capture(op: Op, a: &[u32], rt: &Runtime, result: Result<u32>) -> Result<Vec<Vec<u8>>> {
    if op == Op::QosLookup && result.is_ok() {
        let n = a[0]
            .checked_add(a[4])
            .filter(|&n| n <= 64)
            .ok_or(10022u32)?;
        let ptr = word(rt, a[11])?;
        let table = read(rt, ptr, 8 + n as usize * 24)?;
        return Ok(vec![
            ptr.to_le_bytes().to_vec(),
            read(rt, ptr, qos_size(&table, a)?)?,
        ]);
    }
    slots(op, a, result)?
        .into_iter()
        .map(|(p, n)| {
            let n = if op == Op::Select {
                set_size(&read(rt, p, 4)?)?
            } else {
                n
            };
            read(rt, p, n)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn film_includes_truncated_datagram_and_sender_even_on_error() {
        let a = [1, 100, 3, 0, 200, 300];
        assert_eq!(
            slots(Op::RecvFrom, &a, Err(10040)),
            Ok(vec![(100, 3), (200, 16), (300, 4)])
        );
        assert_eq!(slots(Op::RecvFrom, &a, Err(10035)), Ok(vec![]));
        assert!(slots(Op::RecvFrom, &a, Ok(4)).is_err());
    }
}

type RandomScope = (Arc<dyn Tap>, &'static str, u32);
thread_local! {
    // State operations use a different allocator from Winsock. This scope
    // follows their synchronous call stack only, never a worker or socket.
    static STATE_RANDOM: std::cell::RefCell<Option<RandomScope>> = const { std::cell::RefCell::new(None) };
}
pub(crate) fn with_random<T>(
    tap: &Arc<dyn Tap>,
    name: &'static str,
    thread: u32,
    run: impl FnOnce() -> T,
) -> T {
    struct Restore(Option<RandomScope>);
    impl Drop for Restore {
        fn drop(&mut self) {
            STATE_RANDOM.replace(self.0.take());
        }
    }
    let _restore = Restore(STATE_RANDOM.replace(Some((tap.clone(), name, thread))));
    run()
}
pub(super) fn state_random(len: usize, draw: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
    STATE_RANDOM.with_borrow(|scope| {
        let Some((tap, name, thread)) = scope else {
            return draw();
        };
        let op = format!("Entropy.{name}");
        let args = [len as u32];
        if tap.replaying() {
            let c = tap.next(&op, &args, *thread).ok_or(10022u32)?;
            c.result?;
            let mut outputs = c.outputs;
            if outputs.len() != 1 || outputs[0].len() != len {
                tap.fault(format!("wrong entropy length for {name}"));
                return Err(10022);
            }
            return Ok(outputs.remove(0));
        }
        let bytes = draw();
        tap.record(
            ::film::RAND,
            Call {
                index: 0,
                op,
                args: args.into(),
                thread: *thread,
                result: bytes.as_ref().map(|_| 0).map_err(|e| *e),
                outputs: bytes.as_ref().ok().cloned().into_iter().collect(),
            },
        );
        bytes
    })
}

/// XNetQosServiceLookup uses the ordinary Live task arena. Running its local
/// allocator preserves free/reuse order. Only the fixed service reply is
/// supplied from the film. Release has no output bytes.
pub(crate) fn service_qos(
    tap: &dyn Tap,
    name: &str,
    a: &[u32],
    thread: u32,
    rt: &Runtime,
    value: u32,
) -> u32 {
    let service = name == "XNetQosServiceLookup" && value == 0;
    let fields = || -> Result<Vec<Vec<u8>>> {
        if !service {
            return Ok(vec![]);
        }
        let ptr = word(rt, a[2])?;
        Ok(vec![ptr.to_le_bytes().to_vec(), read(rt, ptr, 32)?])
    };
    if tap.replaying() {
        let Some(c) = tap.next(name, a, thread) else {
            return value;
        };
        let restored = (|| -> Result<()> {
            if c.result != Ok(value) {
                return Err(10022);
            }
            if service {
                let ptr = word(rt, a[2])?;
                let [addr, body] = c.outputs.as_slice() else {
                    return Err(10022);
                };
                if addr.as_slice() != ptr.to_le_bytes().as_slice() || body.len() != 32 {
                    return Err(10022);
                }
                write(rt, ptr, body)?;
            }
            Ok(())
        })();
        if restored.is_err() {
            tap.fault(format!("{name} allocation or return differs"));
        }
    } else {
        match fields() {
            Ok(outputs) => tap.record(
                ::film::NETR,
                Call {
                    index: 0,
                    op: name.into(),
                    args: a.into(),
                    thread,
                    result: Ok(value),
                    outputs,
                },
            ),
            Err(e) => tap.fault(format!("cannot read {name} output ({e})")),
        }
    }
    value
}

/// Only data used by the disconnected network dispatcher survives a seek.
/// The call tape supplies socket outcomes, including last error. Only key IDs
/// are needed by local consumers. Exchange keys never enter this section.
#[derive(serde::Serialize, serde::Deserialize)]
struct FilmState {
    claims: Vec<(u32, u32)>,
    view: crate::film_service::View,
    net: u32,
    wsa: u32,
    key_ids: Vec<[u8; 8]>,
    qos: QosMemory,
}

impl crate::Hle {
    pub fn film_service_taped(&self) -> bool {
        self.service_taped.get().copied().unwrap_or(false)
    }

    pub fn save_film_network(&self, g: &runtime::Guest) -> std::result::Result<Vec<u8>, String> {
        if !self.film_service_taped() {
            return Err("live service calls are not taped".into());
        }
        let side = self.film_side.lock().unwrap_or_else(|e| e.into_inner());
        if side.pending() {
            return Err("Live has an outstanding asynchronous guest output buffer".into());
        }
        let net = lock(&self.network.state);
        let saved = FilmState {
            claims: crate::film_service::claims(g),
            view: side.view.clone(),
            net: net.net,
            wsa: net.wsa,
            key_ids: net.keys.keys().copied().collect(),
            qos: net.qos.clone(),
        };
        serde_json::to_vec(&saved).map_err(|e| e.to_string())
    }

    pub fn load_film_network(
        &self,
        data: &[u8],
        g: &mut runtime::Guest,
    ) -> std::result::Result<(), String> {
        if !self.offline_film
            || !self
                .network
                .tap
                .get()
                .is_some_and(|t| t.service_calls() && t.replaying())
        {
            return Err("Live film state needs the isolated Theater HLE".into());
        }
        if data.len() > 4 << 20 {
            return Err("oversized Live film state".into());
        }
        let saved: FilmState = serde_json::from_slice(data).map_err(|e| e.to_string())?;
        if saved.claims.len() > 8 || saved.key_ids.len() > 64 {
            return Err("invalid Live film state".into());
        }
        saved.qos.validate(&saved.claims)?;
        let claims = crate::film_service::claims(g);
        let missing = saved
            .claims
            .strip_prefix(claims.as_slice())
            .ok_or("Live claim layout differs")?;
        crate::film_service::add_claims(g, missing)?;
        self.film_side
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .view = saved.view;
        let mut net = lock(&self.network.state);
        net.net = saved.net;
        net.wsa = saved.wsa;
        net.keys = saved.key_ids.into_iter().map(|id| (id, [0; 16])).collect();
        net.qos = saved.qos;
        Ok(())
    }
}
