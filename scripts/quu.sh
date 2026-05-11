#!/usr/bin/env bash

# NOTE: This is just a reference implementation. Feel free to make it your own.

# Find the song directory
SONG_DIR=$(grep "music_dir = " ~/.config/tuun/config.toml | cut -d'"' -f2)
SONG_DIR=${SONG_DIR:-${XDG_MUSIC_DIR:-"$HOME/Music"}}

# And ensure it exists
if ! [[ -d $SONG_DIR ]]; then
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
{
    if command -v fd &>/dev/null; then
        fd -tf -e mp3 -e opus -e wav -e m4a -e ogg -e flac --base-directory "$SONG_DIR"
    else
        find "$SONG_DIR" -mindepth 1 -type f \
            \( -iname '*.mp3' -o -iname '*.opus' -o -iname '*.wav' -o -iname '*.m4a' -o -iname '*.ogg' -o -iname '*.flac' \) |
                sed "s,$SONG_DIR/,,"
    fi
} | fzm |
    sed -e "s,^,$SONG_DIR/,"    \
        -e 's,\\,\\\\,g'        \
        -e 's,",\\",g'          \
        > /tmp/tuun/_quu.tpl

# If nothing was selected, remove the queue and exit
if ! [[ -s /tmp/tuun/_quu.tpl ]]; then
    rm -f /tmp/tuun/_quu.tpl
    exit 0
fi

# Finalize the queue if anything was actually selected
mv /tmp/tuun/_quu.tpl /tmp/tuun/quu.tpl

# Start tuun if something was queued and it isn't running
if [[ -e /tmp/tuun/quu.tpl ]] && ! pgrep -x 'tuun' &>/dev/null; then
    alacritty --class tuun --hold -e %BINDIR%/tuun &
fi
