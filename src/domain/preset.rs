use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Tiny,
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tuning {
    pub ram_mb: u32,
    pub max_connections: u32,
    pub shared_buffers_mb: u32,
    pub effective_cache_size_mb: u32,
    pub work_mem_mb: u32,
    pub max_wal_size_mb: u32,
}

impl Preset {
    pub fn tuning(self) -> Tuning {
        match self {
            Preset::Tiny => Tuning {
                ram_mb: 1024,
                max_connections: 50,
                shared_buffers_mb: 256,
                effective_cache_size_mb: 768,
                work_mem_mb: 5,
                max_wal_size_mb: 1024,
            },
            Preset::Small => Tuning {
                ram_mb: 2048,
                max_connections: 100,
                shared_buffers_mb: 512,
                effective_cache_size_mb: 1536,
                work_mem_mb: 5,
                max_wal_size_mb: 2048,
            },
            Preset::Medium => Tuning {
                ram_mb: 4096,
                max_connections: 200,
                shared_buffers_mb: 1024,
                effective_cache_size_mb: 3072,
                work_mem_mb: 5,
                max_wal_size_mb: 4096,
            },
            Preset::Large => Tuning {
                ram_mb: 8192,
                max_connections: 400,
                shared_buffers_mb: 2048,
                effective_cache_size_mb: 6144,
                work_mem_mb: 5,
                max_wal_size_mb: 8192,
            },
        }
    }
}

impl FromStr for Preset {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "tiny" => Ok(Preset::Tiny),
            "small" => Ok(Preset::Small),
            "medium" => Ok(Preset::Medium),
            "large" => Ok(Preset::Large),
            other => Err(format!("unknown preset: {other:?}")),
        }
    }
}
