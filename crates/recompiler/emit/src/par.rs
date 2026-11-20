//! Data parallelism for the per-function stages (lift, IR passes, C
//! generation, file writes): independent work over a slice, on every core,
//! with results in input order so the output is the same as a serial run.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Items handed to a worker at a time: small enough to balance uneven
/// functions, large enough that the cursor isn't contended.
const CHUNK: usize = 16;

fn workers(items: usize) -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).min(items.div_ceil(CHUNK)).max(1)
}

/// `f` over `items`, results in the order of `items`.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = workers(items.len());
    if n == 1 {
        return items.iter().map(f).collect();
    }
    let cursor = AtomicUsize::new(0);
    let done: Mutex<Vec<(usize, Vec<R>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let start = cursor.fetch_add(CHUNK, Ordering::Relaxed);
                    if start >= items.len() {
                        break;
                    }
                    let out: Vec<R> = items[start..(start + CHUNK).min(items.len())].iter().map(&f).collect();
                    done.lock().unwrap().push((start, out));
                }
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_unstable_by_key(|d| d.0);
    done.into_iter().flat_map(|d| d.1).collect()
}

/// `f` on every item of `items`.
pub fn for_each_mut<T: Send>(items: &mut [T], f: impl Fn(&mut T) + Sync) {
    let n = workers(items.len());
    if n == 1 {
        items.iter_mut().for_each(f);
        return;
    }
    let chunks = Mutex::new(items.chunks_mut(CHUNK));
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let Some(chunk) = chunks.lock().unwrap().next() else { break };
                    chunk.iter_mut().for_each(&f);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_keeps_input_order_and_for_each_mut_touches_every_item_once() {
        for len in [0, 1, CHUNK - 1, CHUNK, 10 * CHUNK + 3] {
            let items: Vec<usize> = (0..len).collect();
            assert_eq!(map(&items, |x| x * 2), items.iter().map(|x| x * 2).collect::<Vec<_>>());
            let mut v = items.clone();
            for_each_mut(&mut v, |x| *x += 1);
            assert_eq!(v, items.iter().map(|x| x + 1).collect::<Vec<_>>());
        }
    }
}
