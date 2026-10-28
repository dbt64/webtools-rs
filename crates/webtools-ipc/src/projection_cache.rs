//! Rebuildable presentation cache for the latest Manager feature projection.

use crate::projection::Projection;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "projection.json";
const MAX_CACHE_BYTES: u64 = 3 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Missing,
    Loaded,
    Invalid,
}

pub struct CacheLoad {
    pub projection: Option<Projection>,
    pub status: CacheStatus,
}

#[derive(Debug, Clone)]
pub struct ProjectionCache {
    path: Option<PathBuf>,
}

impl ProjectionCache {
    pub fn new(root: &Path) -> Self {
        Self {
            path: Some(root.join(FILE_NAME)),
        }
    }

    pub fn memory() -> Self {
        Self { path: None }
    }

    pub fn load(&self) -> CacheLoad {
        let Some(path) = &self.path else {
            return CacheLoad {
                projection: None,
                status: CacheStatus::Missing,
            };
        };
        if !path.exists() {
            return CacheLoad {
                projection: None,
                status: CacheStatus::Missing,
            };
        }
        let loaded = (|| {
            let metadata = fs::metadata(path)?;
            if metadata.len() == 0 || metadata.len() > MAX_CACHE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "projection cache size is invalid",
                ));
            }
            let bytes = fs::read(path)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "projection cache JSON is invalid",
                )
            })?;
            crate::projection::parse_projection(&value).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "projection cache schema is invalid",
                )
            })
        })();
        match loaded {
            Ok(projection) => CacheLoad {
                projection: Some(projection),
                status: CacheStatus::Loaded,
            },
            Err(_) => CacheLoad {
                projection: None,
                status: CacheStatus::Invalid,
            },
        }
    }

    pub fn save(&self, projection: &Projection) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let value = crate::projection::projection_cache_value(projection);
        let bytes = serde_json::to_vec(&value)?;
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "projection cache exceeds 3 MiB",
            ));
        }
        crate::state::persist(path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{CacheStatus, ProjectionCache};
    use crate::projection::{AcceptedContext, ContextKind, FeatureRow, Projection};
    use std::fs;

    fn root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "webtools-projection-cache-{}",
            crate::new_request_id()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn projection() -> Projection {
        Projection {
            host_session_id: "11111111-1111-4111-8111-111111111111".into(),
            revision: 7,
            features: vec![FeatureRow {
                plugin_id: "webtools.translation".into(),
                feature_id: "translate-text".into(),
                plugin_name: "翻译".into(),
                title: "翻译".into(),
                commands: vec!["translate".into()],
                accepted_contexts: vec![AcceptedContext {
                    kind: ContextKind::Text,
                    sources: vec!["selected-text".into()],
                    extensions: None,
                    max_items: Some(12),
                    text_match: Some("non-empty".into()),
                }],
                icon_token: "translate".into(),
                feature_kind: "runtime-v3".into(),
                presentation: Some("window".into()),
                icon_data_url: Some("data:image/png;base64,AA==".into()),
            }],
        }
    }

    #[test]
    fn projection_cache_round_trips_manager_projection() {
        let root = root();
        let cache = ProjectionCache::new(&root);
        let expected = projection();

        cache.save(&expected).unwrap();
        let loaded = cache.load();

        assert_eq!(loaded.status, CacheStatus::Loaded);
        assert_eq!(loaded.projection, Some(expected));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_projection_cache_falls_back_to_empty() {
        let root = root();
        fs::write(root.join("projection.json"), b"{").unwrap();
        let loaded = ProjectionCache::new(&root).load();

        assert_eq!(loaded.status, CacheStatus::Invalid);
        assert!(loaded.projection.is_none());
        let _ = fs::remove_dir_all(root);
    }
}
