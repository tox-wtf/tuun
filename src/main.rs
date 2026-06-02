use std::io::ErrorKind as IOE;
use std::path::PathBuf;
use std::process::exit;
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use std::{env, fs};

use config::Config;
use discord_rich_presence::DiscordIpcClient;
use integrations::connect_discord_rpc_client;
use permitit::Permit;
use rustfm_scrobble::Scrobbler;
use tokio::sync::Mutex;
use tracing::{error, info};
use tracing_appender::rolling;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::time::FormatTime;

mod args;
mod config;
mod integrations;
mod mpv;
mod playlists;
mod structs;

pub static CONFIG: LazyLock<Config> = LazyLock::new(Config::load);
pub static ARGS: LazyLock<args::Args> = LazyLock::new(args::parse_args);
pub static RPC_CLIENT: LazyLock<Mutex<DiscordIpcClient>> = LazyLock::new(|| Mutex::new(DiscordIpcClient::new(&CONFIG.discord.client_id)));
pub static SCROBBLER: LazyLock<Mutex<Option<Arc<Scrobbler>>>> = LazyLock::new(|| Mutex::new(None));

/// # Description
/// Main loop (should never return)
///
/// Does stuff in this order:
///     1. Initialize logging
///     2. Create `/tmp/tuun`
///     3. Create `/tmp/tuun/tuun.lock`
///     4. Establish the music directory
///     5. Generate playlists
///     6. Optionally connect to Discord
///     7. Optionally authenticate with `LastFM`
///     8. Launch mpv
///     9. Block forever
#[tokio::main]
async fn main() -> ! {
    // Initialize logging
    let _ = fs::write("/tmp/tuun/log", "");
    let file_appender = rolling::never("/tmp/tuun", "log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);

    let log_level = env::var("TUUN_LOG_LEVEL").unwrap_or_else(|_| String::from("info"));
    let filter = EnvFilter::new(format!("{log_level},rustls=info,ureq=info,winit=info,calloop=info,polling=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_level(true)
        .with_target(true)
        .with_timer(Uptime::new())
        .with_writer(file_writer)
        .with_line_number(true)
        .compact()
        .init();

    info!("Starting tuun");

    // Create /tmp/tuun
    if let Err(e) = fs::create_dir("/tmp/tuun").permit(|e| e.kind() == IOE::AlreadyExists) {
        error!("Failed to create /tmp/tuun: {e}");
        exit(1)
    }

    // Create lock
    if let Err(e) = fs::write("/tmp/tuun/tuun.lock", b"") {
        error!("Failed to write to tuun.lock: {e}");
        exit(1)
    }

    info!("Created lock");

    let music_dir = &CONFIG.general.music_dir;
    if !PathBuf::from(music_dir).exists() {
        error!("Music directory '{music_dir}' does not exist!");
        exit(1)
    }

    // Create auto-generated playlists
    playlists::create_all_playlist();
    playlists::create_recent_playlist();

    // Connect to discord if it's used
    if CONFIG.discord.used {
        connect_discord_rpc_client().await;
    }

    // Authenticate LastFM scrobbler in the background if it's used
    if CONFIG.lastfm.used {
        tokio::spawn(async {
            if let Err(e) = integrations::auth_lastfm_scrobbler().await {
                error!("Error during scrobbler authentication: {e:#?}");
            } else {
                info!("Authenticated with lastfm");
            }
        });
    }

    // Launch mpv
    tokio::spawn(async {
        info!("Launching mpv");
        mpv::launch().await;
        if let Err(e) = mpv::connect().await {
            error!("Failed to connect to mpv's socket: {e:#?}");
        }
    });

    // Hang out forever
    loop {
        std::thread::park();
    }
}

/// # Uptime struct for timestamp formatting in logs
struct Uptime(Instant);

impl Uptime {
    /// # Create a new [`Uptime`]
    #[inline]
    #[must_use]
    fn new() -> Self { Self(Instant::now()) }
}

impl FormatTime for Uptime {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let elapsed = self.0.elapsed();
        let s = elapsed.as_secs();
        let ms = elapsed.subsec_millis();
        write!(w, "{s:>4}.{ms:03}")
    }
}
