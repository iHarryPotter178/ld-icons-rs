mod app;
mod config;
mod layout;
mod nsd;
mod positions;
mod shm;
mod watch;
mod wayland;

use clap::Parser;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
};

use crate::{
    app::App,
    config::{Args, VERSION},
};

fn main() -> io::Result<()> {
    let args = Args::parse();
    if let Some(log) = &args.log {
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(log)?;
        writeln!(file, "ldicons {VERSION} starting")?;
    }
    let app = App::new(&args)?;
    app::run(app, &args)
}
