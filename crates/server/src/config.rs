//! The server's config file: where it listens and which stations may push to it.

use crate::ServerError;
use serde::Deserialize;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

/// Paths a station slug can't take, because the server answers them itself.
const RESERVED_SLUGS: [&str; 3] = ["admin", "api", "status-json.xsl"];
const MAX_SLUG_LEN: usize = 32;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub public_url: String,
    #[serde(default)]
    pub trusted_proxy: Option<IpAddr>,
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
    #[serde(rename = "station", default)]
    pub stations: Vec<Station>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Station {
    pub slug: String,
    pub name: String,
    pub key_hash: String,
    #[serde(default = "default_max_stream_listeners")]
    pub max_stream_listeners: usize,
}

fn default_max_stream_listeners() -> usize {
    200
}

impl Config {
    /// Reads a config from TOML text and checks it.
    pub fn parse(text: &str) -> Result<Self, ServerError> {
        let config: Config =
            toml::from_str(text).map_err(|e| ServerError::Config(e.to_string()))?;
        config.check()?;
        Ok(config)
    }

    /// Reads and checks the config file at `path`.
    pub fn load(path: &Path) -> Result<Self, ServerError> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    /// The station with this slug, if there is one.
    pub fn station(&self, slug: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.slug == slug)
    }

    fn check(&self) -> Result<(), ServerError> {
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            return Err(ServerError::Config(
                "tls_cert and tls_key must be given together".into(),
            ));
        }
        let mut seen = HashSet::new();
        for station in &self.stations {
            check_slug(&station.slug)?;
            if !seen.insert(station.slug.as_str()) {
                return Err(ServerError::Config(format!(
                    "two stations use the slug \"{}\"",
                    station.slug
                )));
            }
        }
        Ok(())
    }
}

/// A slug is one URL path segment: 1–32 of `a-z`, `0-9` and `-`, not starting with `-`,
/// and not a path the server answers itself.
fn check_slug(slug: &str) -> Result<(), ServerError> {
    let well_formed = !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && !slug.starts_with('-')
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !well_formed {
        return Err(ServerError::Config(format!(
            "station slug \"{slug}\" must be 1–{MAX_SLUG_LEN} characters of a-z, 0-9 and -, \
             not starting with -"
        )));
    }
    if RESERVED_SLUGS.contains(&slug) {
        return Err(ServerError::Config(format!(
            "station slug \"{slug}\" is reserved by the server"
        )));
    }
    Ok(())
}
