// src/playlists.rs
//! Some logic for handling playlists

use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;

use tracing::{debug, info, instrument, trace, warn};

use crate::CONFIG;

#[derive(Debug)]
pub struct Playlist {
    playlist_path: PathBuf,
}

impl Playlist {
    #[instrument]
    pub fn new(playlist_path: PathBuf) -> Self {
        let pl = Self { playlist_path };
        debug!("Created new playlist: {pl:#?}");
        pl
    }

    #[instrument(level = "trace")]
    pub fn write(&self, songs: &[PathBuf]) {
        let contents = songs
            .iter()
            .filter(|p| p.is_file())
            .map(|song| song.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&self.playlist_path, &contents).expect("Failed to write playlist");
        debug!("Wrote playlist: {self:#?}");
        trace!("Playlist contents: {contents}");
    }
}

#[instrument]
pub fn create_all_playlist() {
    let path = PathBuf::from("/tmp/tuun/all.tpl");

    // only recreate all.tpl on restarts since it resides in /tmp
    if path.exists() {
        return
    }

    debug!("Creating the all playlist...");
    let all_playlist = Playlist::new(path);

    let mut builder = ignore::WalkBuilder::new(&CONFIG.general.music_dir);
    builder.ignore(false);
    builder.git_ignore(false);
    builder.git_exclude(false);
    builder.hidden(true);
    let walker = builder.build();

    let songs = walker.filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_some_and(|ft| ft.is_file()) &&
                entry.path().extension() == Some(OsStr::new("mp3"))
        })
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();

    debug!("Collected songs: {songs:#?}");

    all_playlist.write(&songs);
    info!("Created the all playlist")
}

#[instrument]
pub fn create_recent_playlist() {
    let path = PathBuf::from("/tmp/tuun/recent.tpl");

    if path.exists() {
        return;
    }

    debug!("Creating the recent playlist...");
    let recent_playlist = Playlist::new(path);

    let mut songs = fs::read_dir(&CONFIG.general.music_dir)
        .expect("Failed to read music directory")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter_map(|e| e.metadata().map_or(None, |m| m.modified().ok().map(|modtime| (e, modtime))))
        .collect::<Vec<_>>();

    songs.sort_by_key(|(_, modtime)| modtime.to_owned());
    let songs: Vec<PathBuf> = songs.iter().rev().map(|(f, _)| f.to_owned()).collect();
    let capped = &songs[..songs.len().min(CONFIG.general.recent_length)];
    recent_playlist.write(capped);
    info!("Created the recent playlist")
}
