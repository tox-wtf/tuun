use std::{
    sync::Arc,
    time::{
        Duration,
        SystemTime,
        UNIX_EPOCH,
    },
};

use anyhow::{
    Result,
    bail,
};
use discord_rich_presence::{
    DiscordIpc,
    activity::{
        self,
        Activity,
        StatusDisplayType,
    },
    error::Error as DrpErr,
};
use permitit::Permit;
use rustfm_scrobble::{
    Scrobble,
    Scrobbler,
};
use tokio::time::timeout;
use tracing::{
    debug,
    error,
    info,
    instrument,
    warn,
};

use crate::{
    CONFIG,
    RPC_CLIENT,
    SCROBBLER,
    structs::{
        LastFM,
        Track,
    },
};

#[instrument]
pub async fn authenticate_lastfm_scrobbler() -> Result<()> {
    let mut scrobbler_lock = SCROBBLER.lock().await;

    if scrobbler_lock.is_none() {
        info!("Authenticating lastfm scrobbler...");
        let lfm = LastFM::new();

        if lfm.apikey.is_empty()
            || lfm.secret.is_empty()
            || lfm.username.is_empty()
            || lfm.password.is_empty()
        {
            warn!("Cowardly refusing to authenticate without credentials");
            bail!("Cowardly refusing to authenticate without credentials");
        }

        let mut scrobbler = Scrobbler::new(lfm.apikey, lfm.secret);
        scrobbler.authenticate_with_password(lfm.username, lfm.password)?;

        *scrobbler_lock = Some(Arc::new(scrobbler));
        drop(scrobbler_lock);
        info!("Authenticated lastfm scrobbler");
    } else {
        debug!("Not authenticating as scrobbler_lock is Some");
    }

    Ok(())
}

#[instrument]
/// Version of [`authenticate_lastfm_scrobbler()`] that doesn't check if the lock is taken
pub async fn authenticate_lastfm_scrobbler_unchecked() -> Result<()> {
    let mut scrobbler_lock = SCROBBLER.lock().await;

    info!("Authenticating lastfm scrobbler...");
    let lfm = LastFM::new();

    if lfm.apikey.is_empty()
        || lfm.secret.is_empty()
        || lfm.username.is_empty()
        || lfm.password.is_empty()
    {
        warn!("Cowardly refusing to authenticate without credentials");
        bail!("Cowardly refusing to authenticate without credentials");
    }

    let mut scrobbler = Scrobbler::new(lfm.apikey, lfm.secret);
    scrobbler.authenticate_with_password(lfm.username, lfm.password)?;

    *scrobbler_lock = Some(Arc::new(scrobbler));
    drop(scrobbler_lock);
    info!("Authenticated lastfm scrobbler");

    Ok(())
}

#[instrument(skip(track))]
pub async fn lastfm_now_playing(track: Track) -> Result<()> {
    let scrobbler_lock = SCROBBLER.lock().await;

    for att in 1..=3 {
        if scrobbler_lock.is_none() {
            warn!("Trying to initialize scrobbler");
            if let Err(e) = authenticate_lastfm_scrobbler_unchecked().await {
                error!("Failed to initialize scrobbler: {e}");
            }
        } else {
            debug!("Got scrobbler lock on attempt {att}");
            break;
        }
    }

    let Some(scrobbler) = &*scrobbler_lock else {
        error!("Scrobbler is not initialized");
        bail!("Scrobbler is not initialized");
    };

    let track = Scrobble::new(&track.get_primary_artist(), &track.title, &track.album);
    let scrobbler = Arc::clone(scrobbler);

    tokio::task::spawn_blocking(move || scrobbler.now_playing(&track)).await??;
    drop(scrobbler_lock);

    debug!("Set now playing");
    Ok(())
}

#[instrument(skip(track))]
pub async fn lastfm_scrobble(track: Track) -> Result<()> {
    let scrobbler_lock = SCROBBLER.lock().await;

    for att in 1..=3 {
        if scrobbler_lock.is_none() {
            warn!("Trying to initialize scrobbler");
            if let Err(e) = authenticate_lastfm_scrobbler_unchecked().await {
                error!("Failed to initialize scrobbler: {e}");
            }
        } else {
            debug!("Got scrobbler lock on attempt {att}");
            break;
        }
    }

    let Some(scrobbler) = &*scrobbler_lock else {
        error!("Scrobbler is not initialized");
        bail!("Scrobbler is not initialized");
    };

    let track = Scrobble::new(&track.get_primary_artist(), &track.title, &track.album);
    let scrobbler = Arc::clone(scrobbler);

    tokio::task::spawn_blocking(move || scrobbler.scrobble(&track)).await??;
    drop(scrobbler_lock);

    debug!("Scrobbled");
    Ok(())
}

#[instrument(skip(track))]
pub async fn discord_rpc(track: Track, now_ago: Duration) -> Result<()> {
    if !CONFIG.discord.used {
        debug!("Skipping discord RPC as Discord is unused in the config");
        return Ok(());
    }

    // TODO: Maybe see if there's a better way of doing this check. For now this is addressed by
    // just lowering the socket connection timeout enough to where the block (which genuinely how
    // is it blocking while on another thread wtf) is not really noticeable.
    //
    // let socketpath = Path::new("/run/user/1000/discord-ipc-0");
    // if !socketpath.exists() {
    //     warn!("Discord IPC {socketpath:?} does not exist");
    //     return Ok(());
    // } // don't complain when discord is closed

    if track.is_default() {
        debug!("Cowardly refusing to set rich presence to a default track");
        return Ok(());
    } // don't try to set empty tracks

    tokio::spawn(async move {
        let mut client = RPC_CLIENT.lock().await;

        if let Err(e) = client.clear_activity() {
            error!("Failed to clear rich presence activity: {e}");

            warn!("Reconnecting Discord RPC client...");
            connect_discord_rpc_client().await;
        }

        let payload = create_rpc_payload(&track, now_ago);
        debug!("Setting Discord Rich Presence for {track:#?}");

        if let Err(e) = client.set_activity(payload) {
            drop(client);
            error!("Failed to set activity: {e}");

            connect_discord_rpc_client().await;
            client = RPC_CLIENT.lock().await;

            let payload = create_rpc_payload(&track, now_ago);
            if let Err(e) = client.set_activity(payload) {
                error!("Failed to set activity after reconnect: {e:#}");
            }
        }

        Ok(())
    })
    .await?
}

#[instrument(skip(track))]
fn create_rpc_payload(track: &Track, now_ago: Duration) -> Activity<'_> {
    debug!("Encoded arturl is 'track.arturl'");

    let small_image = track.artist_pfp.as_ref().unwrap_or(&CONFIG.discord.small_image);
    let small_text = track.get_primary_artist();
    let small_url = track.artist_url.as_deref().unwrap_or("https://github.com/tox-wtf/tuun");

    let assets = activity::Assets::new()
        .large_image(&track.arturl)
        .large_text(&track.album)
        .large_url(&track.arturl)
        .small_image(small_image)
        .small_text(small_text)
        .small_url(small_url);
    debug!("Created rich presence activity assets");

    let now = SystemTime::now();
    let start = now
        .duration_since(UNIX_EPOCH)
        .expect("Grandfather paradox or something idk")
        - now_ago;
    let end = start + Duration::from_secs_f64(track.duration);

    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    let timestamp = activity::Timestamps::new()
        .start(start.as_millis() as i64)
        .end(end.as_millis() as i64);

    let payload = if let (Some(artist_url), Some(srcurl)) = (&track.artist_url, &track.srcurl) {
        activity::Activity::new()
            .state(&track.artist)
            .state_url(artist_url)
            .details(&track.title)
            .details_url(srcurl)
            .assets(assets)
            .activity_type(activity::ActivityType::Listening)
            .status_display_type(StatusDisplayType::Details)
            .timestamps(timestamp)
    } else if let Some(srcurl) = &track.srcurl {
        activity::Activity::new()
            .state(&track.artist)
            .details(&track.title)
            .details_url(srcurl)
            .assets(assets)
            .activity_type(activity::ActivityType::Listening)
            .status_display_type(StatusDisplayType::Details)
            .timestamps(timestamp)
    } else {
        activity::Activity::new()
            .state(&track.artist)
            .details(&track.title)
            .assets(assets)
            .activity_type(activity::ActivityType::Listening)
            .status_display_type(StatusDisplayType::Details)
            .timestamps(timestamp)
    };

    debug!("Created rich presence activity payload");
    payload
}

#[allow(clippy::significant_drop_tightening)]
#[instrument]
// TODO: See if this should be rewritten
pub async fn connect_discord_rpc_client() {
    debug!("Attempting to connect Discord RPC client");

    let time = Duration::from_millis(CONFIG.discord.timeout);
    let lock = timeout(time, RPC_CLIENT.lock()).await;

    let Ok(mut client) = lock else {
        error!("Timed out while trying to acquire lock");
        return;
    };

    // attempt reconnection is recv fails
    if client.recv().is_err() {
        debug!("Failed to receive. Attempting to reconnect client.");
        if let Err(e) = client.close().permit(|e| matches!(e, DrpErr::NotConnected)) {
            error!("Failed to close IPC client: {e}");
            return;
        }
        if let Err(e) = client.connect() {
            error!("Failed to reconnect IPC client: {e}");
            return;
        }
    }

    if let Err(e) = client.connect() {
        error!("Failed to connect to Discord RPC client: {e}");
        return;
    }

    debug!("Connected Discord RPC client");
}
