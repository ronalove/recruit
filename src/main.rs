// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (C) 2026 Ronan Lamour
mod app;
mod board;
mod claude;
mod cli;
mod config;
mod create;
mod i18n;
mod menu;
mod mux;
mod sys;
mod team;
mod ui;
mod update;

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
