use clap::Parser;

/// Tuun: A simple music player using mpv as a backend
#[derive(Parser, Debug)]
#[command(version, about)]
pub struct Args {
    /// Enable shuffling (default: from config)
    #[arg(short, long)]
    pub shuffle: Option<bool>,

    /// Choose playlist (default: from config)
    ///
    /// Example: ~/Music/playlist.tpl
    #[arg(short, long)]
    pub playlist: Option<String>,
}

pub fn parse_args() -> Args { Args::parse() }
