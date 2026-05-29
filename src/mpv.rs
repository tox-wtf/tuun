use std::{
    fs,
    path::PathBuf,
    process::exit,
    sync::{
        Arc,
        LazyLock,
        atomic::{
            AtomicBool,
            AtomicU32,
            Ordering,
        },
    },
};

use anyhow::Result;
use serde_json::Value;
use tokio::{
    io::{
        AsyncBufReadExt,
        AsyncWriteExt,
        BufReader,
    },
    net::UnixStream,
    process::Command,
    sync::Mutex,
    time::{
        Duration,
        sleep,
    },
};
use tracing::{
    debug,
    error,
    info,
    instrument,
    trace,
    warn,
};
use treats::InspectNone;

use crate::{
    ARGS,
    CONFIG,
    integrations::{
        lastfm_now_playing,
        lastfm_scrobble,
    },
    structs::Track,
};

const SOCK_PATH: &str = "/tmp/tuun/mpvsocket";

/// The maximum number of directory entries checked when looking for external cover art
const EXTERNAL_COVER_ART_SEARCH_LIMIT: usize = 128;

pub static LOOPED: AtomicBool = AtomicBool::new(false);
pub static PAUSED: AtomicBool = AtomicBool::new(false);
pub static MUTED: AtomicBool = AtomicBool::new(false);
pub static VOLUME: AtomicU32 = AtomicU32::new(0);

static SCROBBLED: AtomicBool = AtomicBool::new(false);
static NOW_PLAYING_SET: AtomicBool = AtomicBool::new(false);
static EXTERNAL_COVER_ART_SET: AtomicBool = AtomicBool::new(false);

static TRACK: LazyLock<Arc<Mutex<Track>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Track::default())));
static QUEUE: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("/tmp/tuun/quu.tpl"));

pub async fn connect() -> Result<()> {
    // Connect to mpv's socket
    let stream = UnixStream::connect(SOCK_PATH).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // The second parameter is an arbitrary observation ID
    // To find more properties, press 'gr' while hovering over mpv
    let subscriptions = [
        r#"{"command": ["observe_property", 1, "filename"]}"#,
        r#"{"command": ["observe_property", 2, "pause"]}"#,
        r#"{"command": ["observe_property", 3, "loop-file"]}"#,
        r#"{"command": ["observe_property", 4, "mute"]}"#,
        r#"{"command": ["observe_property", 5, "playback-time"]}"#,
        r#"{"command": ["observe_property", 6, "metadata"]}"#,
        r#"{"command": ["observe_property", 7, "volume"]}"#,
    ];

    // Send all subscription commands
    for command in &subscriptions {
        writer.write_all(command.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    // Continuously read lines from mpv's socket
    let mut line = String::with_capacity(4096);
    loop {
        let bytes_read = reader.read_line(&mut line).await?;
        if bytes_read == 0 {
            // EOF
            break;
        }

        match serde_json::from_str::<Value>(&line) {
            | Ok(json) => {
                handle_events(json).await;
            },
            | Err(e) => {
                error!("Failed to parse JSON: {e}");
            },
        }
        line.clear();
    }

    Ok(())
}

#[instrument(skip(command), level = "debug")]
pub async fn send_command<S: AsRef<str>>(command: S) -> Result<Value> {
    let stream = UnixStream::connect(SOCK_PATH).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    debug!("Connected to mpv socket {SOCK_PATH:?}");

    let command = command.as_ref();
    writer.write_all(command.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    trace!("Sent mpv command: {command:#}");

    let mut response = String::with_capacity(64);
    reader.read_line(&mut response).await?;

    let json: Value = serde_json::from_str(&response)?;
    trace!("Received mpv response: {json:#}");

    Ok(json)
}

/// Handles mpv events
///
/// Supported events include start-file, end-file, and property-change.
async fn handle_events(json: Value) {
    if let Some(event) = json.get("event").and_then(|v| v.as_str()) {
        match event {
            | "start-file" => {
                debug!("mpv event: New file started");
            },
            | "end-file" => {
                if let Some(reason) = json.get("reason").and_then(|v| v.as_str()) {
                    if reason == "quit" {
                        info!("mpv quit. Exiting...");
                        exit(0)
                    } else {
                        debug!("mpv event: EOF:\n{reason:#}");
                    }
                }
            },
            | "property-change" => {
                trace!("Detected property change: {json:#}");
                handle_properties(json).await;
            },
            | _ => {
                trace!("mpv event: Received uncategorized event:\n{event:#}");
            },
        }
    }
}

/// Handles mpv properties
///
/// Supported properties include filename, pause, loop-file, mute, and playback-time.
#[instrument(level = "trace")]
async fn handle_properties(json: Value) {
    if let Err(e) = queue().await {
        error!("Failed to refresh queue: {e:#}");
    }

    if let Some(property) = json.get("name").and_then(Value::as_str) {
        match property {
            | "filename" => {
                debug!("Filename changed");
                debug!("Filename property: {json:#}");
            },
            | "pause" => {
                debug!("Pause property: {json:#}");
                if let Some(paused) = json.get("data").and_then(Value::as_bool) {
                    PAUSED.store(paused, Ordering::Relaxed);
                    if paused {
                        info!("Paused");
                    } else {
                        info!("Unpaused");
                    }
                }
            },
            | "metadata" => {
                debug!("mpv property: Metadata changed");
                debug!("Metadata property: {json:#}");

                let mut track = TRACK.lock().await;
                if let Err(e) = track.update_metadata(&json).await {
                    error!("Failed to update metadata: {e:#?}");
                }

                drop(track);
            },
            | "loop-file" => {
                debug!("Loop property: {json:#}");
                if let Some(looped) = json.get("data") {
                    let looped = match looped {
                        | Value::Bool(b) => *b,
                        | Value::String(s) => s == "inf",
                        | _ => false,
                    };
                    LOOPED.store(looped, Ordering::Relaxed);
                    if looped {
                        info!("Looped");
                    } else {
                        info!("Unlooped");
                    }
                }
            },
            | "volume" => {
                debug!("Volume: {json:#}");

                if let Some(vol) = json.get("data").and_then(Value::as_f64) {
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    let vol = vol.trunc() as u32;
                    VOLUME.store(vol, Ordering::Relaxed);
                    info!("Volume set to {vol}");
                }
            },
            | "mute" => {
                debug!("Mute property: {json:#}");
                if let Some(muted) = json.get("data").and_then(Value::as_bool) {
                    MUTED.store(muted, Ordering::Relaxed);
                    if muted {
                        info!("Muted");
                    } else {
                        info!("Unmuted");
                    }
                }
            },
            | "playback-time" => {
                trace!("mpv property: Playback time changed");
                let mut track = TRACK.lock().await;
                let time = json.get("data").and_then(Value::as_f64).unwrap_or(0.);
                trace!("Time: {time}");

                // If a track is fresh, it can be scrobbled and its now playing status has not yet
                // been set. It may also need external album art to be set.
                if time == 0.0 {
                    debug!("Track is fresh");
                    SCROBBLED.store(false, Ordering::Relaxed);
                    NOW_PLAYING_SET.store(false, Ordering::Relaxed);
                    EXTERNAL_COVER_ART_SET.store(false, Ordering::Relaxed);
                }

                track.update_progress(time);
                track.display();

                if !EXTERNAL_COVER_ART_SET.load(Ordering::Relaxed) {
                    use_external_cover_art().await;
                    EXTERNAL_COVER_ART_SET.store(true, Ordering::Relaxed);
                }

                // Set now playing status if the track has been playing for more than a
                // configureable delay, or it's more than 5% through.
                #[allow(clippy::cast_precision_loss)]
                let delay =
                    (track.duration * 0.05).min(CONFIG.general.now_playing_delay as f64 / 1000.);

                if time >= delay && !NOW_PLAYING_SET.load(Ordering::Relaxed) {
                    NOW_PLAYING_SET.store(true, Ordering::Relaxed);
                    info!("Now playing '{track}'");
                    debug!("Pushing now playing status");

                    if CONFIG.lastfm.used {
                        info!("Setting LastFM now playing");
                        let track_copy = track.clone();
                        // TODO: Consider making `lastfm_now_playing` spawn its own thread rather
                        // than having the caller do it
                        tokio::spawn(async move {
                            if let Err(e) = lastfm_now_playing(track_copy).await {
                                error!("Failed to set LastFM now playing: {e:#?}");
                            }
                        });
                    }

                    if CONFIG.discord.used {
                        info!("Setting Discord Rich Presence");
                        track.rpc(Duration::from_secs_f64(delay)).await;
                    }
                }

                // Scrobble track if it's more than a configurable percent through.
                if !SCROBBLED.load(Ordering::Relaxed)
                    && time >= (track.duration * (f64::from(CONFIG.lastfm.scrobble_percent) / 100.))
                {
                    SCROBBLED.store(true, Ordering::Relaxed);

                    if CONFIG.lastfm.used {
                        // TODO: Implement display for track so the logs look nicer
                        info!("Scrobbling track: {track:#?}");
                        let track_copy = track.clone();
                        drop(track);
                        tokio::spawn(async move {
                            if let Err(e) = lastfm_scrobble(track_copy).await {
                                error!("Failed to scrobble track: {e:#?}");
                            }
                        });
                    }
                }
            },
            | _ => {
                warn!("mpv property: Received unrecognized property:\n{json:#}");
            },
        }
    }
}

/// Return a path to the currently playing track, according to mpv
pub async fn get_filepath() -> Option<PathBuf> {
    let Ok(data) = send_command(r#" { "command" : [ "get_property", "path" ] } "#).await else {
        warn!("Failed to get path");
        return None;
    };

    let Some(data) = data.as_object() else {
        warn!("mpv returned invalid JSON");
        return None;
    };

    let filename = data.get("data")?.as_str()?;
    Some(PathBuf::from(filename))
}

/// Return a path to this track's external cover art, if any
///
/// This cover art may be either animated or static, and should exist in the same directory as the
/// track
pub async fn get_external_cover() -> Option<PathBuf> {
    debug!("Checking if external cover art exists");

    let mut filepath: Option<PathBuf> = None;
    for _ in 0..4 {
        filepath = get_filepath().await;
        if filepath.is_some() {
            break
        }
    }

    let filepath = filepath?;
    let parent = filepath.parent()?;

    parent
        .read_dir()
        .ok()?
        .take(EXTERNAL_COVER_ART_SEARCH_LIMIT)
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|s| s.starts_with("cover"))
        })
        .map(|entry| entry.path())
}

pub async fn use_external_cover_art() -> Option<()> {
    let external_cover = get_external_cover()
        .await
        .inspect_none(|| debug!("No external cover art found"))?;
    info!("Attempting to use external cover art at {external_cover:?}");

    // album art flag passed to mpv; yes means static
    let album_art_flag = match external_cover.extension()?.to_str()? {
        | "mp4" | "gif" => "no",
        | _ => "yes",
    };

    debug!("Adding video track for external cover art");
    send_command(format!(
        // url, flags, title, lang, album_art_flag
        r#"{{ "command": ["video-add", "{}", "", "", "{}"] }}"#,
        external_cover.to_str()?,
        album_art_flag,
    ))
    .await
    .ok()?;

    let json = send_command(r#"{ "command": ["get_property", "track-list"] }"#)
        .await
        .ok()?;

    let tracks = json.get("data").and_then(|j| j.as_array())?.clone();
    let final_video_track = tracks.iter().rfind(|track| {
        track
            .get("type")
            .is_some_and(|j| j.as_str() == Some("video"))
            && track.get("title").is_some_and(|j| {
                j.as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("artist picture"))
            })
    })?;

    send_command(format!(
        r#"{{ "command": ["set_property", "vid", {}] }}"#,
        final_video_track.get("id")?.as_u64()?
    ))
    .await
    .ok()?;

    debug!("Added video track for external cover art");
    Some(())
}

#[instrument]
pub async fn launch() {
    info!("Launching mpv...");
    let to_shuffle: &str =
        if ARGS.shuffle.unwrap_or(CONFIG.general.shuffle) { "yes" } else { "no" };

    let mut mpv = Command::new("mpv")
        .arg(format!("--shuffle={to_shuffle}"))
        .arg("--really-quiet")
        .arg("--geometry=350x350+1400+80")
        .arg("--title=tuun-mpv")
        .arg("--loop-playlist=inf")
        .arg(format!("--input-ipc-server={SOCK_PATH}"))
        .args(prequeue())
        .spawn()
        .expect("Failed to launch mpv");
    let pid = mpv.id();

    // Record tuun-mpv's pid, but don't whine if something goes wrong
    if let Some(i) = pid {
        let _ = fs::write("/tmp/tuun/tuun-mpv.pid", i.to_string());
    }

    for a in 1..=32 {
        sleep(Duration::from_millis(
            CONFIG.general.mpv_socket_poll_timeout as u64,
        ))
        .await;
        debug!("Polling mpv socket {a}/32...");
        if fs::metadata(SOCK_PATH).is_ok() {
            debug!("mpv socket was ok on attempt {a}");
            break;
        }
    }

    if let Ok(optcode) = mpv.try_wait()
        && let Some(code) = optcode
        && !code.success()
    {
        error!("mpv exited with a failure");
        if ARGS.playlist.is_some() {
            error!("This is most likely caused by your playlist referencing inaccessible tracks");
        }
    }

    match queue().await {
        | Ok(queued) => {
            if queued {
                info!("Starting with queued tracks");
                if let Err(e) = send_command(r#"{ "command": ["playlist-next"] }"#).await {
                    error!("Failed to skip track for queue start: {e}");
                }
            }
        },
        | Err(e) => error!("Failed to queue tracks from start: {e}"),
    }
}

#[instrument]
fn prequeue() -> Vec<String> {
    // FIXME: This can probably be written less grossly(?)
    let playlist = &ARGS
        .playlist
        .clone()
        .unwrap_or_else(|| CONFIG.general.playlist.clone());

    debug!("Starting with playlist '{playlist}'");
    if !PathBuf::from(playlist).exists() {
        error!("Playlist '{playlist}' does not exist");
        panic!("Playlist '{playlist}' does not exist");
    }

    let args = if QUEUE.exists() {
        debug!("Queue.tpl exists");
        vec![
            format!("--playlist={}", QUEUE.display()),
            format!("--playlist={playlist}"),
        ]
    } else {
        vec![format!("--playlist={playlist}")]
    };

    debug!("Prequeue args for mpv: {args:#?}");
    args
}

#[instrument]
async fn queue() -> Result<bool> {
    let queue = &*QUEUE;

    trace!("Checking whether queue {queue:?} exists...");
    if !queue.exists() {
        trace!("No songs queued");
        return Ok(false);
    }

    let songs = fs::read_to_string(queue)?;
    for song in songs.lines() {
        let song = song.trim();
        let command = format!(r#"{{ "command": ["loadfile", "{song}", "insert-next"] }}"#);
        send_command(&command).await?;
        info!("Queued {song}");
    }

    fs::remove_file(queue)?;
    debug!("Removed queue file {queue:?}");
    Ok(true)
}
