//! The server's config file: where it listens and which stations may push to it.

use crate::ServerError;
use crate::key::parse_hash;
use crate::pairing::PublicUrl;
use radio_codecs::encode::{Mp3Encoder, OpusEncoder};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

/// Paths a station slug can't take, because the server answers them itself.
const RESERVED_SLUGS: [&str; 3] = ["admin", "api", "status-json.xsl"];
const MAX_SLUG_LEN: usize = 32;
/// Paths under a station an output's mount can't take, because the server answers them
/// itself (`/<slug>/stream` is the pushed bytes, untouched).
const FIXED_MOUNTS: [&str; 4] = ["stream", "live", "signal", "feed"];
const MAX_MOUNT_LEN: usize = 32;
/// The most outputs a station may be given from the app (`PUT /api/station/outputs`) and
/// so the most a saved file may hold. The config file itself has no such limit.
pub const MAX_SAVED_OUTPUTS: usize = 8;

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
    /// A directory the server writes: each station's outputs set from the app are saved in
    /// it as `<slug>.outputs.json`, and replace the station's config outputs at start.
    /// Without it, outputs come from this file only and the app can't change them.
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
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
    /// The re-encoded streams the server makes from what the station pushes, in config order.
    #[serde(rename = "output", default)]
    pub outputs: Vec<Output>,
}

/// One re-encoded stream of a station, served at `/<slug>/<mount>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub mount: String,
    pub format: OutputFormat,
    pub kbps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Opus,
    Mp3,
}

impl OutputFormat {
    /// The name the config file uses for this format.
    pub fn name(self) -> &'static str {
        match self {
            OutputFormat::Opus => "opus",
            OutputFormat::Mp3 => "mp3",
        }
    }
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
        PublicUrl::parse(&self.public_url).map_err(ServerError::Config)?;
        let mut seen = HashSet::new();
        // The hash each station's key has, to the first station with it.
        let mut hashes = HashMap::new();
        for station in &self.stations {
            check_slug(&station.slug)?;
            if !seen.insert(station.slug.as_str()) {
                return Err(ServerError::Config(format!(
                    "two stations use the slug {:?}",
                    station.slug
                )));
            }
            // A message about a key hash names the station, never the value: it may be
            // the key itself, pasted in by mistake.
            let Some(hash) = parse_hash(&station.key_hash) else {
                return Err(ServerError::Config(format!(
                    "station {:?}: key_hash must be sha256: and 64 hex digits, as \
                     `craftradio-server key` prints it",
                    station.slug
                )));
            };
            if let Some(first) = hashes.insert(hash, station.slug.as_str()) {
                return Err(ServerError::Config(format!(
                    "stations {first:?} and {:?} have the same key_hash; each station \
                     needs a key of its own",
                    station.slug
                )));
            }
            check_outputs(station)?;
        }
        Ok(())
    }
}

/// Each output of a station has a mount of its own and a bitrate its encoder can make.
fn check_outputs(station: &Station) -> Result<(), ServerError> {
    check_output_list(&station.outputs)
        .map_err(|what| ServerError::Config(format!("station {:?}, {what}", station.slug)))
}

/// What [`check_outputs`] checks, for a bare list: the reason in words, naming the output.
fn check_output_list(outputs: &[Output]) -> Result<(), String> {
    let mut mounts = HashSet::new();
    for output in outputs {
        let fail = |what: String| format!("output {:?}: {what}", output.mount);
        check_mount(&output.mount).map_err(fail)?;
        if !mounts.insert(output.mount.as_str()) {
            return Err(fail("two outputs of the station use this mount".into()));
        }
        // The encoder is the one judge of which bitrates it makes, so the server keeps no
        // table of its own that could drift from it.
        let made = match output.format {
            OutputFormat::Opus => OpusEncoder::new(output.kbps).map(drop),
            OutputFormat::Mp3 => Mp3Encoder::new(output.kbps).map(drop),
        };
        made.map_err(|e| {
            fail(format!(
                "{} at {} kbps can't be made: {e}",
                output.format.name(),
                output.kbps
            ))
        })?;
    }
    Ok(())
}

/// Checks a list of outputs set from the app: what the config checks, and at most
/// [`MAX_SAVED_OUTPUTS`]. The reason in words.
pub fn check_saved_outputs(outputs: &[Output]) -> Result<(), String> {
    if outputs.len() > MAX_SAVED_OUTPUTS {
        return Err(format!(
            "a station may have at most {MAX_SAVED_OUTPUTS} outputs; this list has {}",
            outputs.len()
        ));
    }
    check_output_list(outputs)
}

/// Reads JSON text of a list of outputs set from the app and checks it.
pub fn parse_saved_outputs(text: &[u8]) -> Result<Vec<Output>, String> {
    let outputs: Vec<Output> = serde_json::from_slice(text).map_err(|e| {
        format!("the outputs must be a JSON list of {{\"mount\", \"format\", \"kbps\"}}: {e}")
    })?;
    check_saved_outputs(&outputs)?;
    Ok(outputs)
}

/// Reads and checks a station's saved outputs file. An error names the file.
pub fn read_saved_outputs(path: &Path) -> Result<Vec<Output>, ServerError> {
    let fail = |what: String| ServerError::Config(format!("{}: {what}", path.display()));
    let text = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    parse_saved_outputs(&text).map_err(fail)
}

/// Where the outputs of station `slug` are saved in `state_dir`.
pub fn saved_outputs_path(state_dir: &Path, slug: &str) -> PathBuf {
    state_dir.join(format!("{slug}.outputs.json"))
}

/// A mount is one URL path segment: 1–32 of `a-z`, `0-9`, `-`, `.` and `_`, not starting
/// with `-` or `.`, and not a path the server answers itself.
fn check_mount(mount: &str) -> Result<(), String> {
    let well_formed = !mount.is_empty()
        && mount.len() <= MAX_MOUNT_LEN
        && !mount.starts_with(['-', '.'])
        && mount.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
        });
    if !well_formed {
        return Err(format!(
            "a mount must be 1–{MAX_MOUNT_LEN} characters of a-z, 0-9, -, . and _, not \
             starting with - or ."
        ));
    }
    if FIXED_MOUNTS.contains(&mount) {
        return Err("this path is the server's own; pick another mount".into());
    }
    Ok(())
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
            "station slug {slug:?} must be 1–{MAX_SLUG_LEN} characters of a-z, 0-9 and -, \
             not starting with -"
        )));
    }
    if RESERVED_SLUGS.contains(&slug) {
        return Err(ServerError::Config(format!(
            "station slug {slug:?} is reserved by the server"
        )));
    }
    Ok(())
}
