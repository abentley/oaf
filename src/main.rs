// Copyright 2021-2022 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
#![cfg_attr(feature = "strict", deny(warnings))]
use clap::Parser;
use std::env;
use std::path::PathBuf;
use std::process::exit;

mod commands;
mod git;
mod worktree;
use commands::{NativeCommand, RunExit};

fn is_oaf_cmd(args_vec: &[String]) -> bool {
    let x = NativeCommand::try_parse_from(&args_vec[0..2]);
    if let Err(e) = x {
        if let clap::error::ErrorKind::UnknownArgument | clap::error::ErrorKind::InvalidSubcommand =
            e.kind()
        {
            return false;
        }
    }
    true
}

fn extract_cmd(progname: &str) -> Option<&str> {
    match progname.split_once('-') {
        Some(("git", _)) => None,
        Some((_, cmd)) => Some(cmd),
        _ => None,
    }
}

/**
 * If the args are not an oaf command, but might be a git command, return None.
 *
 * Otherwise, return the result of parsing args as a NativeCommand.
 */
fn parse_args(args_vec: &Vec<String>) -> Option<NativeCommand> {
    let mut args_iter = args_vec.clone().into_iter();
    let progpath = PathBuf::from(args_iter.next().expect("Invoked with 0 arguments"));
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "oaf" => {
            if args_vec.len() > 1 && !is_oaf_cmd(args_vec) {
                return None;
            }
            NativeCommand::parse()
        }
        _ => {
            let Some(cmd) = extract_cmd(progname) else {
                eprintln!("Unsupported command name {}", progname);
                exit(1);
            };
            let mut args = vec!["oaf".to_string(), cmd.to_string()];
            args.extend(args_iter);
            NativeCommand::parse_from(args)
        }
    };
    Some(opt)
}

fn main() {
    let args_vec = env::args().collect();
    let Some(args) = parse_args(&args_vec) else {
        args_vec[1..].to_owned().run_exec();
    };
    args.run_exec();
}
