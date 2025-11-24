// config.rs
//
// responsible for handling config.toml
//
// NOTE: My dumb ass spent entirely too long failing to figure out how to just use impl Default for
// structs with missing fields. The solution is to just use #[serde(default)] at the struct level.
// https://users.rust-lang.org/t/serde-default-versus-impl-default/66773
// https://serde.rs/container-attrs.html#default

use std::{
    env,
    fs,
    path::Path,
};

use serde::Deserialize;
use tracing::{
    debug,
    error,
    info,
    warn,
};

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Config {
    pub lastfm:  LastFMConfig,
    pub discord: DiscordConfig,
    pub general: GeneralConfig,
    pub color:   ColorConfig,
}

impl Default for LastFMConfig {
    fn default() -> Self {
        Self {
            used:             false,
            apikey:           String::new(),
            secret:           String::new(),
            user:             String::new(),
            password:         String::new(),
            scrobble_percent: 44,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct LastFMConfig {
    pub used:             bool,
    pub apikey:           String,
    pub secret:           String,
    pub user:             String,
    pub password:         String,
    pub scrobble_percent: u8,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            used:         true,
            client_id:    "1272345557276295310".to_owned(),
            fallback_art: "https://w7.pngwing.com/pngs/387/453/png-transparent-phonograph-record-lp-record-45-rpm-album-concerts-miscellaneous-photography-sound-thumbnail.png".to_owned(),
            small_image:  "https://cdn.discordapp.com/avatars/495603896803262507/4c3f854b1aa44e41850908d06c17bd25".to_owned(), // TODO: Add tuun logo
            small_text:   format!("tuun {}", env!("CARGO_PKG_VERSION")),
            small_url:    "https://github.com/tox-wtf/tuun".to_owned(),
            timeout:      100,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct DiscordConfig {
    pub used:         bool,
    pub client_id:    String,
    pub fallback_art: String,
    pub small_image:  String,
    pub small_text:   String,
    pub small_url:    String,
    /// Timeout in milliseconds for discord ipc socket connections
    pub timeout:      u64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ColorConfig {
    pub primary:   String,
    pub secondary: String,
    pub tertiary:  String,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            primary:   "#cdcddd".into(),
            secondary: "#333333".into(),
            tertiary:  "#e5e5f5".into(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(default)]
pub struct GeneralConfig {
    pub artists_with_commas:     Vec<String>,
    pub shuffle:                 bool,
    pub playlist:                String,
    pub music_dir:               String,
    pub recent_length:           usize,
    pub mpv_socket_poll_timeout: usize,
    pub now_playing_delay:       usize,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            artists_with_commas:     vec![
                "Tyler, The Creator".into(),
                "Everything, Now!".into(),
                "Wow, Owls!".into(),
                "Go Robot, Go!".into(),
                "What Price, Wonderland?".into(),
                "Hey, Revolution!".into(),
                "Muira,".into(),
            ],
            shuffle:                 true,
            playlist:                "/tmp/tuun/all.tpl".to_owned(),
            music_dir:               get_fallback_music_dir()
                .expect("Couldn't retrieve fallback music directory"),
            recent_length:           350,
            mpv_socket_poll_timeout: 96,
            now_playing_delay:       2345,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let home_dir = homedir::my_home()
            .expect("Couldn't find home directory")
            .expect("Couldn't find home directory");
        debug!("Detected home directory: '{}'", home_dir.display());

        let config_dir = get_fallback_config_dir().expect("Couldn't find config directory");
        let config_path = Path::new(&config_dir).join("tuun/config.toml");

        if !config_path.exists() {
            warn!("Creating default config at {}", config_path.display());
            Self::create_default(&config_path);
        }

        let Ok(config_str) = fs::read_to_string(&config_path) else {
            error!("Couldn't find config.toml at {}", config_path.display());
            panic!()
        };

        let mut config: Self = match toml::de::from_str(&config_str) {
            | Ok(c) => c,
            | Err(e) => {
                error!("Invalid syntax detected in config");
                panic!("{e}");
            },
        };

        // allow ~ and $HOME in paths in config.toml
        let home_dir_str = home_dir.to_string_lossy();
        config.general.music_dir = config.general.music_dir.replacen('~', &home_dir_str, 1);
        config.general.playlist = config.general.playlist.replacen('~', &home_dir_str, 1);
        config.general.music_dir = config.general.music_dir.replacen("$HOME", &home_dir_str, 1);
        config.general.playlist = config.general.playlist.replacen("$HOME", &home_dir_str, 1);

        info!("Loaded config");
        debug!("Config: {config:#?}");
        config
    }

    fn create_default(config_path: &Path) {
        let datadir = option_env!("DATADIR").unwrap_or("/usr/share/data");
        let default_config_path = Path::new(datadir).join("default_config.toml");
        info!(
            "Copying default config from {} to {}",
            default_config_path.display(),
            config_path.display()
        );

        let config_dir = config_path
            .parent()
            .expect("Config dir's parent should exist");
        if !config_dir.exists() {
            fs::create_dir_all(config_dir).expect("Failed to create config directory");
            info!("Created config directory at '{}'", config_dir.display());
        }

        if let Err(e) = fs::copy(&default_config_path, config_path) {
            error!(
                "Failed to copy default config from '{}' to '{}': {e}",
                default_config_path.display(),
                config_path.display()
            );
            warn!("Did you run make install?");
        }
    }
}

/// This function retrieves a fallback config directory.
fn get_fallback_config_dir() -> Option<String> {
    if let Ok(config_dir) = env::var("XDG_CONFIG_HOME") {
        return Some(config_dir)
    }

    if let Some(config_dir) =
        env::home_dir().map(|p| p.join(".config").to_string_lossy().to_string())
    {
        return Some(config_dir)
    }

    None
}

/// This function retrieves a fallback music directory.
///
/// Note that this should only be used to find a default fallback music directory if not set in the
/// config.
fn get_fallback_music_dir() -> Option<String> {
    if let Ok(music_dir) = env::var("XDG_MUSIC_DIR") {
        return Some(music_dir)
    }

    if let Some(music_dir) = env::home_dir().map(|p| p.join("Music").to_string_lossy().to_string())
    {
        return Some(music_dir)
    }

    None
}
