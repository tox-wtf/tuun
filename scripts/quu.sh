#!/usr/bin/env sh

# NOTE: This is just a reference implementation. Feel free to make it your own.

# Find the song directory
SONG_DIR=$(grep "music_dir = " ~/.config/tuun/config.toml | cut -d'"' -f2)
SONG_DIR=${SONG_DIR:-${XDG_MUSIC_DIR:-"$HOME/Music"}}

# And ensure it exists
if [ ! -d "$SONG_DIR" ]; then
    echo "Music directory not found: $SONG_DIR" >&2
    exit 1
fi

mkdir -p /tmp/tuun

# Gather selected songs
#
# Then, write them to the queue, and apply some fixes:
# 1. Prepend the song directory
# 2. Change \ to \\ to appease mpv
# 3. Change " to \" to appease mpv
find "$SONG_DIR" -mindepth 1 -type f \
    \( -iname '*.mp3' -o -iname '*.opus' -o -iname '*.wav' -o -iname '*.m4a' -o -iname '*.ogg' -o -iname '*.flac' \) |
    sed 's,.*/,,'       | # strip full path
    shuf                | # shuffle
    fzm                 | # write queue
    sed -e "s,^,$SONG_DIR/,"    \
        -e 's,\\,\\\\,g'        \
        -e 's,",\\",g'          \
        > /tmp/tuun/_quu.tpl

# Finalize the queue if anything was actually selected; otherwise remove the
# temporary queue
if [ -s /tmp/tuun/_quu.tpl ]; then
    mv /tmp/tuun/_quu.tpl /tmp/tuun/quu.tpl
else
    rm -f /tmp/tuun/_quu.tpl
fi

# Start tuun if something was queued and it isn't running
if [ -e /tmp/tuun/quu.tpl ] && ! pgrep -x 'tuun' >/dev/null 2>&1; then
    alacritty --class tuun --hold -e %BINDIR%/tuun &
fi
