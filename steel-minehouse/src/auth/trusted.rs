//! Local copy of trusted nickname + IP pairs, used only while the site is unreachable: our
//! players keep getting in, new ones are turned away.

use std::{
    fs, io,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rustc_hash::FxHashMap;
use sha2::{Digest, Sha256};
use steel_utils::locks::SyncMutex;

/// Trusted pairs as `sha256(nickname|ip)` → expiry (Unix milliseconds), the Fabric mod's
/// `minehouse-auth-trusted.json`. The nickname is case-sensitive.
pub struct TrustedCache {
    file: PathBuf,
    ttl: Duration,
    expiries: SyncMutex<FxHashMap<String, u64>>,
}

impl TrustedCache {
    /// Reads `file` if it exists. A `ttl` of zero turns the cache off.
    #[must_use]
    pub fn load(file: PathBuf, ttl: Duration) -> Self {
        let expiries = match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log::warn!(
                    "Could not read {}, starting with an empty cache: {error}",
                    file.display()
                );
                FxHashMap::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => FxHashMap::default(),
            Err(error) => {
                log::warn!(
                    "Could not read {}, starting with an empty cache: {error}",
                    file.display()
                );
                FxHashMap::default()
            }
        };
        Self {
            file,
            ttl,
            expiries: SyncMutex::new(expiries),
        }
    }

    /// Whether the pair was confirmed recently enough.
    #[must_use]
    pub fn is_trusted(&self, nickname: &str, ip: &str) -> bool {
        self.expiries
            .lock()
            .get(&key(nickname, ip))
            .is_some_and(|expires_at| *expires_at > now_millis())
    }

    /// Remembers the pair for the configured time.
    pub fn trust(&self, nickname: &str, ip: &str) {
        if self.ttl.is_zero() {
            return;
        }
        let mut expiries = self.expiries.lock();
        let expires_at = now_millis().saturating_add(self.ttl.as_millis() as u64);
        expiries.insert(key(nickname, ip), expires_at);
        self.save(&mut expiries);
    }

    /// Drops the pair, e.g. after a ban or leaving Discord.
    pub fn forget(&self, nickname: &str, ip: &str) {
        let mut expiries = self.expiries.lock();
        if expiries.remove(&key(nickname, ip)).is_some() {
            self.save(&mut expiries);
        }
    }

    fn save(&self, expiries: &mut FxHashMap<String, u64>) {
        let now = now_millis();
        expiries.retain(|_, expires_at| *expires_at > now);
        if let Err(error) = self.write(expiries) {
            log::warn!("Could not save {}: {error}", self.file.display());
        }
    }

    fn write(&self, expiries: &FxHashMap<String, u64>) -> io::Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.file.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string(expiries)?)?;
        fs::rename(tmp, &self.file)
    }
}

fn key(nickname: &str, ip: &str) -> String {
    hex::encode(Sha256::digest(format!("{nickname}|{ip}").as_bytes()))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    #[test]
    fn trusted_pairs_survive_a_reload_and_can_be_forgotten() {
        let dir = env::temp_dir().join(format!("steel-minehouse-trusted-{}", process::id()));
        let file = dir.join("auth-trusted.json");
        let ttl = Duration::from_secs(3600);

        let cache = TrustedCache::load(file.clone(), ttl);
        assert!(!cache.is_trusted("Steve", "1.2.3.4"));
        cache.trust("Steve", "1.2.3.4");

        let reloaded = TrustedCache::load(file.clone(), ttl);
        assert!(reloaded.is_trusted("Steve", "1.2.3.4"));
        assert!(!reloaded.is_trusted("steve", "1.2.3.4"));
        assert!(!reloaded.is_trusted("Steve", "1.2.3.5"));
        // Same key as the Fabric mod writes, so its file can be carried over.
        assert_eq!(
            key("Steve", "1.2.3.4"),
            hex::encode(Sha256::digest(b"Steve|1.2.3.4"))
        );

        reloaded.forget("Steve", "1.2.3.4");
        assert!(!TrustedCache::load(file, ttl).is_trusted("Steve", "1.2.3.4"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn zero_ttl_turns_the_cache_off() {
        let file = env::temp_dir().join(format!("steel-minehouse-trusted-off-{}", process::id()));
        let cache = TrustedCache::load(file, Duration::ZERO);
        cache.trust("Steve", "1.2.3.4");
        assert!(!cache.is_trusted("Steve", "1.2.3.4"));
    }
}
