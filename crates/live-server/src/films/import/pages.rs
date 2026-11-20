//! Immutable stats pages and their content-addressed seek directory.
use super::{ApiResult, BlobStore, Meta, Row, unavailable};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const INDEX_FANOUT: usize = 32;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Page {
    pub(super) sha256: String,
    pub(super) count: usize,
    // Existing durable selections used sequence-number-only object names.
    #[serde(default)]
    pub(super) hash_key: bool,
}

/// Level zero holds data pages. A full level is stored as one immutable
/// directory and carried into the next level. The signed root consequently
/// holds at most 31 descriptors per level irrespective of match length.
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct PageIndex {
    levels: Vec<Vec<Page>>,
    pub(super) len: usize,
}

impl PageIndex {
    pub(super) async fn push(
        &mut self,
        store: &BlobStore,
        blob: &str,
        mut descriptor: Page,
    ) -> ApiResult<()> {
        let mut level = 0;
        loop {
            if self.levels.len() == level {
                self.levels.push(Vec::new());
            }
            let children = &mut self.levels[level];
            children.push(descriptor);
            if children.len() < INDEX_FANOUT {
                break;
            }
            let bytes = serde_json::to_vec(children).expect("stats page directory");
            let sha256 = digest(&bytes);
            store
                .put(
                    &format!("{blob}.import/index-{sha256}"),
                    bytes.into(),
                    &Meta::default(),
                )
                .await
                .map_err(unavailable)?;
            children.clear();
            descriptor = Page {
                sha256,
                count: 0,
                hash_key: true,
            };
            level += 1;
        }
        self.len += 1;
        Ok(())
    }

    pub(super) async fn page(
        &self,
        store: &BlobStore,
        blob: &str,
        mut n: usize,
    ) -> ApiResult<Page> {
        if n >= self.len {
            return Err(unavailable("stats page outside selected directory"));
        }
        for (level, nodes) in self.levels.iter().enumerate().rev() {
            let mut width = INDEX_FANOUT
                .checked_pow(level as u32)
                .ok_or_else(|| unavailable("invalid stats directory depth"))?;
            let slot = n / width;
            if slot >= nodes.len() {
                n -= nodes.len() * width;
                continue;
            }
            let mut node = nodes[slot].clone();
            n %= width;
            while width > 1 {
                let bytes = store
                    .get(&format!("{blob}.import/index-{}", node.sha256))
                    .await
                    .map_err(unavailable)?
                    .ok_or_else(|| unavailable("missing stats directory"))?;
                if digest(&bytes) != node.sha256 {
                    return Err(unavailable("stats directory digest mismatch"));
                }
                let children: Vec<Page> = serde_json::from_slice(&bytes).map_err(unavailable)?;
                if children.len() != INDEX_FANOUT {
                    return Err(unavailable("invalid stats directory width"));
                }
                width /= INDEX_FANOUT;
                node = children[n / width].clone();
                n %= width;
            }
            return Ok(node);
        }
        Err(unavailable("inconsistent stats directory"))
    }
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn page_key(blob: &str, n: usize, page: &Page) -> String {
    if page.hash_key {
        format!("{blob}.import/{n}-{}", page.sha256)
    } else {
        format!("{blob}.import/{n}")
    }
}

pub(super) async fn read_page(
    store: &BlobStore,
    blob: &str,
    n: usize,
    page: &Page,
) -> ApiResult<Vec<Row>> {
    let bytes = store
        .get(&page_key(blob, n, page))
        .await
        .map_err(unavailable)?
        .ok_or_else(|| unavailable("missing stats page"))?;
    if digest(&bytes) != page.sha256 {
        return Err(unavailable("stats page digest mismatch"));
    }
    let rows: Vec<Row> = serde_json::from_slice(&bytes).map_err(unavailable)?;
    if rows.len() != page.count {
        return Err(unavailable("stats page length mismatch"));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_compacted_page_directory_survives_restart_and_checks_nested_digests() {
        let store = BlobStore::memory();
        let blob = "private-film";
        let mut index = PageIndex::default();
        for n in 0..INDEX_FANOUT.pow(2) + 5 {
            index
                .push(
                    &store,
                    blob,
                    Page {
                        sha256: n.to_string(),
                        count: n + 1,
                        hash_key: true,
                    },
                )
                .await
                .unwrap();
        }
        assert_eq!(
            index.levels.iter().map(Vec::len).collect::<Vec<_>>(),
            [5, 0, 1]
        );
        let index: PageIndex =
            serde_json::from_slice(&serde_json::to_vec(&index).unwrap()).unwrap();
        for n in [0, 31, 32, 1023, 1024, 1028] {
            let leaf = index.page(&store, blob, n).await.unwrap();
            assert_eq!(leaf.sha256, n.to_string());
            assert_eq!(leaf.count, n + 1);
        }
        assert!(index.page(&store, blob, index.len).await.is_err());
        let root = &index.levels[2][0];
        store
            .put(
                &format!("{blob}.import/index-{}", root.sha256),
                vec![0].into(),
                &Meta::default(),
            )
            .await
            .unwrap();
        assert!(index.page(&store, blob, 0).await.is_err());
        assert!(index.page(&store, blob, 1024).await.is_ok());
    }
}
