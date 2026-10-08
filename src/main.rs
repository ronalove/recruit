// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
mod app;
mod board;
mod bridge;
mod canvas;
mod claude;
mod cli;
mod config;
mod generate;
mod i18n;
mod launch;
mod layout;
mod live;
mod look;
mod member;
mod menu;
mod prompt;
mod state;
mod templates;
mod tmux;
mod ui;
mod wizard;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    i18n::init(cli::lang_from_args(&args));
    let cli = cli::parse(args);
    match app::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.is::<ui::Cancelled>() => {
            eprintln!("{}", t!("Annulé.", "Cancelled."));
            ExitCode::from(130)
        }
        Err(error) => {
            eprintln!("recruit: {error:#}");
            ExitCode::FAILURE
        }
    }
}
