//! Rebuildable indexes bound to one immutable committed sequence.
use artex_core::{Error, Record, Result, Snapshot};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
#[derive(Debug, Default)]
pub struct QueryIndex {
    values: BTreeMap<(String, String, String), BTreeSet<String>>,
}
impl QueryIndex {
    pub fn build(state: &Snapshot) -> Self {
        let mut values = BTreeMap::new();
        for (collection, records) in &state.collections {
            for r in records.values() {
                if let Some(o) = r.data.as_object() {
                    for (field, v) in o {
                        if !v.is_object() && !v.is_array() {
                            values
                                .entry((collection.clone(), field.clone(), v.to_string()))
                                .or_insert_with(BTreeSet::new)
                                .insert(r.header.id.clone());
                        } else if let Some(a) = v.as_array() {
                            for item in a.iter().filter(|i| i.is_string()) {
                                values
                                    .entry((collection.clone(), field.clone(), item.to_string()))
                                    .or_insert_with(BTreeSet::new)
                                    .insert(r.header.id.clone());
                            }
                        }
                    }
                }
            }
        }
        Self { values }
    }
    pub fn ids(&self, collection: &str, field: &str, value: &Value) -> Option<&BTreeSet<String>> {
        self.values
            .get(&(collection.into(), field.into(), value.to_string()))
    }
    pub fn entry_count(&self) -> usize {
        self.values.len()
    }
}
#[derive(Debug)]
pub struct ReadGeneration {
    pub snapshot: Arc<Snapshot>,
    pub indexes: QueryIndex,
}
impl std::ops::Deref for ReadGeneration {
    type Target = Snapshot;
    fn deref(&self) -> &Snapshot {
        &self.snapshot
    }
}
#[derive(Debug)]
pub struct SearchPage {
    pub commit_seq: String,
    pub records: Vec<Record>,
    pub next_after: Option<String>,
    pub scanned: usize,
}
impl ReadGeneration {
    pub fn new(snapshot: Arc<Snapshot>) -> Self {
        let indexes = QueryIndex::build(&snapshot);
        Self { snapshot, indexes }
    }
    pub fn lookup(
        &self,
        collection: &str,
        field: &str,
        value: &Value,
        after: Option<&str>,
        limit: usize,
    ) -> Result<SearchPage> {
        artex_core::collection_prefix(collection)?;
        if limit == 0 || limit > 1000 {
            return Err(Error::new("INVALID_PARAMS", "Query limit must be 1..1000"));
        }
        let mut out = Vec::new();
        let mut more = false;
        if let Some(ids) = self.indexes.ids(collection, field, value) {
            for id in ids
                .iter()
                .filter(|id| after.is_none_or(|a| id.as_str() > a))
            {
                if out.len() == limit {
                    more = true;
                    break;
                }
                out.push(self.snapshot.get(collection, id).unwrap().clone());
            }
        }
        let next_after = if more {
            out.last().map(|r| r.header.id.clone())
        } else {
            None
        };
        Ok(SearchPage {
            commit_seq: self.snapshot.commit_seq.to_string(),
            records: out,
            next_after,
            scanned: 0,
        })
    }
    /// Exact text search is bounded per page. The cursor is the last scanned ID,
    /// including when the scan budget is exhausted before finding `limit` rows.
    pub fn search(
        &self,
        collection: &str,
        text: &str,
        after: Option<&str>,
        limit: usize,
        scan_budget: usize,
    ) -> Result<SearchPage> {
        artex_core::collection_prefix(collection)?;
        if limit == 0 || limit > 1000 || scan_budget == 0 || scan_budget > 100_000 {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Invalid search page or scan budget",
            ));
        }
        let q = text.to_lowercase();
        let mut out = Vec::new();
        let mut scanned = 0;
        let mut last = None;
        let mut more = false;
        for r in self
            .snapshot
            .records(collection)
            .into_iter()
            .filter(|r| after.is_none_or(|a| r.header.id.as_str() > a))
        {
            if out.len() == limit || scanned == scan_budget {
                more = true;
                break;
            }
            scanned += 1;
            last = Some(r.header.id.clone());
            if r.data.to_string().to_lowercase().contains(&q) {
                out.push(r.clone());
            }
        }
        Ok(SearchPage {
            commit_seq: self.snapshot.commit_seq.to_string(),
            records: out,
            next_after: if more { last } else { None },
            scanned,
        })
    }
    pub fn typed<T: artex_core::models::Entity>(
        &self,
        id: &artex_core::models::Id<T>,
    ) -> Result<artex_core::models::TypedRecord<T>> {
        let r = self
            .snapshot
            .get(T::COLLECTION, id.as_str())
            .ok_or_else(|| Error::new("NOT_FOUND", "Entity not found"))?;
        artex_core::models::TypedRecord::<T>::try_from_record(r)
    }
}
