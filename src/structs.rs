use std::{
    fmt,
    io::{
        self,
        Write,
    },
    path::PathBuf,
    sync::atomic::Ordering,
    time::Duration,
};

use anyhow::{
    Context,
    Result,
    bail,
};
use id3::{
    Content,
    Tag,
    TagLike,
};
use serde_json::Value;
use tracing::{
    debug,
    error,
    instrument,
    trace,
    warn,
};
use urlencoding::encode;

use crate::{
    CONFIG,
    config::ColorConfig,
    integrations,
    mpv::{
        LOOPED,
        MUTED,
        VOLUME,
        send_command,
    },
};

#[derive(Debug, Clone)]
pub struct Track {
    pub arturl:     String,
    pub album_url:  Option<String>,
    pub artist_url: Option<String>,
    pub artist_pfp: Option<String>,
    pub srcurl:     Option<String>,
    pub title:      String,
    pub artist:     String,
    pub album:      String,
    pub date:       String,
    pub progress:   f64,
    pub duration:   f64,
}

impl Default for Track {
    fn default() -> Self {
        Self {
            arturl:     String::new(),
            album_url:  None,
            artist_url: None,
            artist_pfp: None,
            srcurl:     None,
            title:      String::new(),
            artist:     String::new(),
            album:      String::new(),
            date:       String::new(),
            progress:   0.0,
            duration:   1000.,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LastFM<'lfm> {
    pub apikey:   &'lfm str,
    pub secret:   &'lfm str,
    pub username: &'lfm str,
    pub password: &'lfm str,
}

impl LastFM<'_> {
    pub fn new() -> Self {
        Self {
            apikey:   &CONFIG.lastfm.apikey,
            secret:   &CONFIG.lastfm.secret,
            username: &CONFIG.lastfm.user,
            password: &CONFIG.lastfm.password,
        }
    }

    #[inline]
    pub fn is_default(&self) -> bool { *self == LastFM::default() }
}

pub fn strip_null(s: &str) -> String { s.replace('\0', "") }

pub fn urlencode(url: &str) -> String {
    let (proto, rest_of_url) = url
        .find("://")
        .map_or(("", url), |index| (&url[..index + 3], &url[index + 3..]));
    let encoded_rest = rest_of_url
        .split('/')
        .map(|part| encode(part).into_owned())
        .collect::<Vec<_>>()
        .join("/");
    format!("{proto}{encoded_rest}")
}

impl Track {
    /// Returns the primary artist, accounting for configured exceptions
    pub fn get_primary_artist(&self) -> String {
        if !self.artist.contains(',') {
            return self.artist.clone()
        }

        for exception in &CONFIG.general.artists_with_commas {
            if self.artist.starts_with(exception) {
                return exception.clone()
            }
        }

        self.artist
            .split_once(',')
            .expect("Handled by a check at the start of the function")
            .0
            .trim()
            .to_string()
    }

    // TODO: See if this should be used anywhere
    // pub async fn query_metadata(&self) -> Result<Value> {
    //     let command = r#" { "command" : [ "get_property", "metadata" ] "#;
    //     send_command(command).await
    // }

    pub async fn query_filepath(&self) -> Result<PathBuf> {
        let command = r#" { "command" : [ "get_property", "path" ] } "#;
        let Ok(data) = send_command(command).await else {
            warn!("Failed to query path");
            bail!("Failed to query path");
        };

        let Some(data) = data.as_object() else {
            warn!("MPV returned invalid JSON");
            bail!("Invalid JSON");
        };

        let filename = data
            .get("data")
            .and_then(|v| v.as_str())
            .context("Filename not present")?;
        Ok(PathBuf::from(filename))
    }

    #[instrument(skip(tag))]
    pub fn get_organization(tag: &Tag) -> Option<String> {
        tag.get("TPUB").map(|t| t.content().to_string())
    }

    #[instrument(skip(data, tag))]
    pub fn get_arturl(data: &serde_json::Map<String, Value>, tag: Option<&Tag>) -> Option<String> {
        debug!("Attempting to get cover art URL");
        if let Some(url) = data.get("arturl").and_then(|v| v.as_str()) {
            debug!("Using key 'arturl' from mpv's metadata");
            return Some(url.to_string());
        }

        if let Some(tag) = tag {
            let arturl = tag.frames().find_map(|f| match f.content() {
                | Content::ExtendedLink(l) if l.description.eq_ignore_ascii_case("front cover") => {
                    debug!("Found under front cover");
                    Some(l.link.clone())
                },
                | Content::ExtendedLink(l) if l.description.eq_ignore_ascii_case("cover") => {
                    debug!("Found under cover");
                    Some(l.link.clone())
                },
                | Content::ExtendedText(t) if t.description == "arturl" => {
                    debug!("Found under deprecated extended text 'arturl'");
                    warn!("Deprecated metadata format for cover art URL");
                    Some(strip_null(&t.value))
                },
                | _ => None,
            });

            if arturl.is_none() {
                warn!("Couldn't find arturl in extended text or cover in extended link frames");
            }

            return arturl;
        }

        None
    }

    #[instrument(skip(tag))]
    pub fn get_artisturl(tag: Option<&Tag>) -> Option<String> {
        debug!("Attempting to get primary artist URL");

        if let Some(tag) = tag {
            let artist_url = tag.frames().find_map(|f| match f.content() {
                | Content::ExtendedLink(l) if l.description.eq_ignore_ascii_case("artist homepage") => {
                    Some(l.link.clone())
                }
                | _ if f.id() == "WOAR" => f.content().link().map(ToString::to_string), // preferred
                | _ => None,
            });

            if artist_url.is_none() {
                warn!("Couldn't find artist_url in any link frames");
            }

            return artist_url;
        }

        None
    }

    #[instrument(skip(tag))]
    pub fn get_albumurl(tag: Option<&Tag>) -> Option<String> {
        debug!("Attempting to get album URL");

        if let Some(tag) = tag {
            let album_url = tag.frames().find_map(|f| match f.content() {
                | Content::ExtendedLink(l) if l.description.eq_ignore_ascii_case("album homepage") => {
                    Some(l.link.clone())
                }
                | _ if f.id() == "WOAS" => f.content().link().map(ToString::to_string), // preferred
                | _ => None,
            });

            if album_url.is_none() {
                warn!("Couldn't find album_url in any link frames");
            }

            return album_url;
        }

        None
    }

    #[instrument(skip(tag))]
    pub fn get_artistpfp(tag: Option<&Tag>) -> Option<String> {
        debug!("Attempting to get primary artist profile picture");

        if let Some(tag) = tag {
            let artist_pfp = tag.frames().find_map(|f| match f.content() {
                | Content::ExtendedLink(l)
                    if l.description.eq_ignore_ascii_case("artist picture") =>
                {
                    debug!("Found under artist picture");
                    Some(l.link.clone())
                },
                | _ => None,
            });

            if artist_pfp.is_none() {
                warn!("Couldn't find artist_url in extended link frames");
            }

            return artist_pfp;
        }

        None
    }

    #[instrument(skip(data, tag))]
    pub fn get_srcurl(data: &serde_json::Map<String, Value>, tag: Option<&Tag>) -> Option<String> {
        if let Some(url) = data.get("srcurl").and_then(|v| v.as_str()) {
            debug!("Using key 'srcurl' from mpv's metadata");
            return Some(url.to_string());
        }

        let tag = tag?;
        if let Some(f) = tag.frames().find(|f| f.id() == "WOAF") {
            return f.content().link().map(ToString::to_string); // preferred
        }

        // fallbacks
        let srcurl = tag.frames().find_map(|f| match f.content() {
            | Content::ExtendedLink(l) if l.description == "Source" => Some(l.link.clone()),
            | Content::ExtendedText(t) if t.description == "srcurl" => {
                Some(strip_null(&t.value))
            },
            | _ if f.id() == "WOAS" => f.content().link().map(ToString::to_string),
            | _ => None,
        });

        if srcurl.is_none() {
            warn!("Couldn't find srcurl in extended text or cover in extended link frames");
        }

        srcurl
    }

    #[instrument(skip(data, tag))]
    pub fn get_artists(data: &serde_json::Map<String, Value>, tag: Option<&Tag>) -> String {
        if let Some(tag) = tag {
            let artist = tag.artist();
            match artist {
                | Some(a) => return a.split('\0').collect::<Vec<_>>().join(", "),
                | None => return String::from("<Unknown artist>"),
            }
        }

        data.get("artist")
            .and_then(|v| v.as_str())
            .unwrap_or("<Unknown artist>")
            .to_string()
    }

    #[instrument(skip(self))]
    pub async fn read_id3_tag(&self) -> Option<Tag> {
        let mut filepath: Option<PathBuf> = None;
        for _ in 0..4 {
            if let Ok(path) = self.query_filepath().await {
                filepath = Some(path);
                break;
            }
        }

        if filepath.as_ref().is_none_or(|f| {
            f.extension()
                .is_some_and(|e| !e.eq_ignore_ascii_case("mp3"))
        }) {
            return None
        }

        Tag::read_from_path(filepath?).ok()
    }

    #[instrument(level = "debug", skip(self, metadata))]
    pub async fn update_metadata(&mut self, metadata: &Value) -> Result<()> {
        let Some(data) = metadata.get("data").and_then(|d| d.as_object()) else {
            // This typically only happens in edge cases where no metadata exists yet, such as when
            // MPV has just started
            debug!("Failed to get metadata from {metadata:#}");
            debug!("Not updating metadata");
            return Ok(());
        };

        let data: serde_json::Map<String, Value> = data
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();

        let mut filepath: Option<PathBuf> = None;
        for _ in 0..4 {
            if let Ok(path) = self.query_filepath().await {
                filepath = Some(path);
                break;
            }
        }

        if filepath.is_none() {
            warn!("Couldn't query filepath; expect inaccurate metadata");
        }

        let tag = self.read_id3_tag().await;

        self.title = data
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("<Unknown title>")
            .to_string();
        self.artist = Self::get_artists(&data, tag.as_ref());
        self.album = data
            .get("album")
            .and_then(|v| v.as_str())
            .unwrap_or("<Unknown album>")
            .to_string();
        self.date = data
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or("<Unknown date>")
            .to_string();

        self.arturl = Self::get_arturl(&data, tag.as_ref())
            .map_or_else(|| CONFIG.discord.fallback_art.clone(), |u| urlencode(&u));

        self.srcurl = Self::get_srcurl(&data, tag.as_ref()).map(|u| urlencode(&u));
        self.artist_url = Self::get_artisturl(tag.as_ref()).map(|u| urlencode(&u));
        self.album_url = Self::get_albumurl(tag.as_ref()).map(|u| urlencode(&u));
        self.artist_pfp = Self::get_artistpfp(tag.as_ref()).map(|u| urlencode(&u));

        debug!("Attempting to find duration");
        // duration is not technically metadata but i count it as such
        let mut duration = None;
        for a in 0..u16::MAX {
            duration = send_command(r#"{"command": ["get_property", "duration"]}"#)
                .await?
                .get("data")
                .and_then(Value::as_f64);
            if let Some(dur) = duration {
                debug!("Fetched duration {dur} on attempt {a}");
                break;
            }
        }

        let Some(dur) = duration else {
            error!("Failed to fetch duration after {} attempts", u16::MAX);
            warn!("Not updating metadata");
            return Ok(());
        };

        self.duration = dur;

        Ok(())
    }

    pub fn update_progress(&mut self, progress: f64) {
        self.progress = progress;
        trace!("Updated progress to {progress}");
    }

    fn format_metadata(&self) -> String {
        let loop_display = if LOOPED.load(Ordering::Relaxed) { " (looped)" } else { "" };
        let mute_display = if MUTED.load(Ordering::Relaxed) { " (muted)" } else { "" };

        let theme = Theme::from(&CONFIG.color);

        format!(
            "\
{b}{p}TUUN {ver}{r}

{b}{p}01 {s}{sep} Ttl - {t}{title}{r}
{b}{p}02 {s}{sep} Art - {t}{artist}{r}
{b}{p}03 {s}{sep} Alb - {t}{album}{r}
{b}{p}04 {s}{sep} Dte - {t}{date}{r}
{b}{p}05 {s}{sep} Vol - {t}{volume}{muted}{r}
{b}{p}06 {s}{sep} Prg - {t}{progress:.3}/{duration:.3}{looped}{r}",
            p = theme.p,
            s = theme.s,
            t = theme.t,
            r = "\x1b[0m",
            b = "\x1b[1m",
            sep = ":::",
            ver = env!("CARGO_PKG_VERSION"),
            title = self.title,
            artist = self.artist,
            album = self.album,
            date = self.date,
            volume = VOLUME.load(Ordering::Relaxed),
            muted = mute_display,
            progress = self.progress,
            duration = self.duration,
            looped = loop_display,
        )
    }

    #[instrument]
    pub fn display(&self) {
        trace!("Displaying track:\n{self:#?}");
        if let Err(e) = cls() {
            warn!("Failed to cls: {e}");
            return;
        }

        let out = self.format_metadata();
        print!("{out}");

        if let Err(e) = io::stdout().flush() {
            warn!("Failed to print metadata: {e:#?}");
        }
    }

    #[instrument]
    pub async fn rpc(&self, now_ago: Duration) {
        if let Err(e) = integrations::discord_rpc(self.clone(), now_ago).await {
            error!("Error setting discord rpc: {e:#?}");
        }
    }

    #[rustfmt::skip]
    pub fn is_default(&self) -> bool {
        self.progress == 0.
            && (self.duration - 1000.0).abs() < 1.0
            && self.title    == String::new()
            && self.artist   == String::new()
            && self.album    == String::new()
            && self.arturl   == String::new()
            && self.date     == String::new()
    }
}

impl fmt::Display for Track {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} - {}", self.artist, self.title)
    }
}

fn hex_to_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.trim_start_matches('#');
    match hex.len() {
        | 3 => {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            Some((r, g, b))
        },
        | 6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        },
        | _ => None,
    }
}

fn fg_rgb(r: u8, g: u8, b: u8) -> String { format!("\x1b[38;2;{r};{g};{b}m") }

fn fg_hex(hex: &str) -> String {
    if let Some((r, g, b)) = hex_to_rgb(hex) {
        fg_rgb(r, g, b)
    } else {
        String::new()
    }
}

#[derive(Debug)]
pub struct Theme {
    /// primary
    pub p: String,
    /// secondary
    pub s: String,
    /// tertiary
    pub t: String,
}

impl From<&ColorConfig> for Theme {
    fn from(cfg: &ColorConfig) -> Self {
        Self {
            p: fg_hex(&cfg.primary),
            s: fg_hex(&cfg.secondary),
            t: fg_hex(&cfg.tertiary),
        }
    }
}

fn cls() -> io::Result<()> {
    print!("{esc}[2J{esc}[1;1H{esc}[?251", esc = 27 as char);
    io::stdout().flush()
}
