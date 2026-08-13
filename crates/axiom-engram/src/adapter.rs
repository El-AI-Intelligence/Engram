//! EngramStore adapter — implements `MemoryBackend` for `EngramStore`.
//!
//! This thin adapter lets `QemCache<EngramStoreAdapter>` wrap the existing
//! SQLCipher vault so the L1 holographic cache can warm from L2 and provide
//! associative lookups with real hit-rate tracking.

use crate::engram::{Engram, EngramLayer, LinkType};
use crate::entry::{
    MemoryEntry, MemoryId, MemoryLayer, MemoryLink,
};
use crate::r#trait::{DecayReport, ConsolidationReport, MemoryBackend, Query};
use crate::store::{EngramStore, TemporalPattern};
use crate::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Adapter that implements `MemoryBackend` by delegating to `EngramStore`.
pub struct EngramStoreAdapter {
    store: Arc<Mutex<EngramStore>>,
}

impl EngramStoreAdapter {
    pub fn new(store: Arc<Mutex<EngramStore>>) -> Self {
        Self { store }
    }
}

// ── Conversion helpers ──────────────────────────────────────────────────────

fn engram_to_memory_entry(e: Engram) -> MemoryEntry {
    MemoryEntry::from(e)
}

fn memory_entry_to_engram(m: MemoryEntry) -> Engram {
    Engram::from(m)
}

fn memory_layer_to_engram_layer(l: MemoryLayer) -> EngramLayer {
    match l {
        MemoryLayer::Episodic => EngramLayer::Episodic,
        MemoryLayer::Semantic => EngramLayer::Semantic,
        MemoryLayer::Imagined => EngramLayer::Imagined,
    }
}

// ── MemoryBackend impl ──────────────────────────────────────────────────────

#[async_trait]
impl MemoryBackend for EngramStoreAdapter {
    async fn capture(&self, entry: MemoryEntry) -> Result<MemoryId> {
        let engram = memory_entry_to_engram(entry);
        let id = MemoryId::from_string(engram.id.clone());
        let store = self.store.lock().await;
        store.write(&engram).await?;
        Ok(id)
    }

    async fn retrieve(&self, id: &MemoryId) -> Result<Option<MemoryEntry>> {
        let store = self.store.lock().await;
        match store.get(id.as_str()).await {
            Ok(engram) => Ok(Some(engram_to_memory_entry(engram))),
            Err(crate::EngramError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn search(&self, query: Query) -> Result<Vec<MemoryEntry>> {
        let store = self.store.lock().await;
        let limit = query.limit;

        let engrams = if let Some(layer) = query.layer {
            store.search_by_layer(memory_layer_to_engram_layer(layer), limit).await?
        } else if let Some(ref text) = query.text {
            store.search_by_content(text, limit).await?
        } else if !query.tags.is_empty() {
            let tag_refs: Vec<&str> = query.tags.iter().map(|t| t.as_str()).collect();
            store.search_by_tags(&tag_refs, limit).await?
        } else {
            store.list(limit, query.offset).await?
        };

        Ok(engrams.into_iter().map(engram_to_memory_entry).collect())
    }

    async fn link(
        &self,
        source: &MemoryId,
        target: &MemoryId,
        link_type: LinkType,
        weight: f64,
    ) -> Result<()> {
        let store = self.store.lock().await;
        store.link(source.as_str(), target.as_str(), weight, link_type).await
    }

    async fn get_links(&self, id: &MemoryId) -> Result<Vec<MemoryLink>> {
        let store = self.store.lock().await;
        let links = store.get_links(id.as_str()).await?;
        Ok(links.into_iter().map(|l| MemoryLink {
            target_id: MemoryId::from_string(l.target_id),
            weight: l.weight,
            link_type: l.link_type,
        }).collect())
    }

    async fn related(&self, id: &MemoryId, limit: usize) -> Result<Vec<MemoryEntry>> {
        let store = self.store.lock().await;
        let engrams = store.search_related(id.as_str(), limit).await?;
        Ok(engrams.into_iter().map(engram_to_memory_entry).collect())
    }

    async fn apply_decay(&self) -> Result<DecayReport> {
        let store = self.store.lock().await;
        let (strengthened, decayed) = store.apply_daily_hygiene().await?;
        Ok(DecayReport {
            strengthened: strengthened as u32,
            decayed: decayed as u32,
            pruned: 0, // EngramStore's daily hygiene doesn't report pruning separately
        })
    }

    async fn consolidate(&self) -> Result<ConsolidationReport> {
        let store = self.store.lock().await;
        let (promoted, pruned) = store.apply_weekly_consolidation().await?;
        Ok(ConsolidationReport {
            promoted_to_semantic: promoted as u32,
            pruned_imagined: pruned as u32,
            narratives_updated: 0,
            rules_crystallized: 0,
        })
    }

    async fn surface(&self, context: &str, limit: usize) -> Result<Vec<(MemoryEntry, f64)>> {
        let store = self.store.lock().await;
        let results = store.surface_relevant(context, limit).await?;
        Ok(results.into_iter().map(|(e, score)| (engram_to_memory_entry(e), score)).collect())
    }

    async fn detect_patterns(
        &self,
        query: &str,
        min_samples: usize,
    ) -> Result<Option<TemporalPattern>> {
        let store = self.store.lock().await;
        store.detect_temporal_patterns(query, min_samples).await
    }

    async fn count(&self) -> Result<u64> {
        let store = self.store.lock().await;
        Ok(store.count().await? as u64)
    }

    async fn store_embedding(&self, id: &MemoryId, embedding: &[f64]) -> Result<()> {
        let store = self.store.lock().await;
        store.store_embedding(id.as_str(), embedding).await
    }

    async fn vector_search(
        &self,
        embedding: &[f64],
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f64)>> {
        let store = self.store.lock().await;
        let results = store.vector_search(embedding, limit).await?;
        Ok(results.into_iter().map(|(e, score)| (engram_to_memory_entry(e), score)).collect())
    }
}
