# Tuun

## Info
Tuun is my music player. Compatibility isn't really a goal, but it should work
on most Linux distros with some finagling. This is its fifth iteration,
featuring async!

## Features
Tuun currently supports at least the following fun features:
- Discord Rich Presence
- LastFM scrobbling and now playing
- Playlists
- Queueing
- Configuration

## Installation
**Incomplete List of Dependencies**
- rust      -> Build
- mpv       -> Runtime. Used to play and control music and display album art.
- fzf       -> Optional. Used in `./scripts/quu.sh`.
- alacritty -> Optional. Used in `./scripts/quu.sh`.

I've decided to use Makefiles to simplify stuff. This should be all it takes:
```bash
./configure
make
sudo make install
```

## Usage
For basic usage, just run `tuun`. For more advanced usage, check out the scripts
in `./scripts`.

Thanks to mpv's socket, you can make hotkeys to control pretty much every aspect
of tuun, which you can pair with `./scripts/mpv.sh`. `./scripts/quu.sh` works
with `./scripts/fzm` to make queueing songs nicer. `./scripts/tuun.sh` wraps
launching and closing `tuun`. Note that `./scripts/mpv.sh` is not installed by
the Makefile. That one's up to you to place where you'd like and configure.

You may also want to make keybinds and window class/title configurations for
`tuun` and `quu` with your window manager.

## Uninstallation
If for whatever reason you're not convinced (and you still have the sources):
```bash
sudo make uninstall
```
