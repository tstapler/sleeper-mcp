use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub base_url: String,
    pub rate_limit_per_minute: usize,
    pub timeout_secs: u64,
    pub cache_ttl_secs: i64,
    pub data_dir: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = std::env::var("SLEEPER_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("cache"));
        Self {
            base_url: "https://api.sleeper.app/v1".to_string(),
            rate_limit_per_minute: 1000,
            timeout_secs: 30,
            cache_ttl_secs: 300,
            data_dir,
        }
    }
}

impl Config {
    pub fn stats_db_path(&self) -> PathBuf {
        self.data_dir.join("stats.db")
    }

    pub fn cache_db_path(&self) -> PathBuf {
        self.data_dir.join("http_cache.db")
    }

    pub fn rate_limit_db_path(&self) -> PathBuf {
        self.data_dir.join("rate_limit.db")
    }

    /// Persistent Chrome profile dir — cookies/localStorage here keep a browser login alive
    /// across runs, so `sleeper browser login` only has to happen once.
    pub fn chrome_profile_dir(&self) -> PathBuf {
        self.data_dir.join("chrome-profile")
    }

    /// Chrome-for-Testing binary cache root (`<data_dir>/cft/`). The pinned
    /// browser lives here — never inside the profile dir, never migrated.
    pub fn cft_dir(&self) -> PathBuf {
        self.data_dir.join("cft")
    }

    /// Version-scoped CfT cache dir (`<data_dir>/cft/<version>/`).
    pub fn cft_version_dir(&self, version: &str) -> PathBuf {
        self.cft_dir().join(version)
    }
}
