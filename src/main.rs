// Copyright 2021 Aaron Bentley <aaron@aaronbentley.com>
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

#[derive(Debug, Parser)]
#[command()]
enum Opt {
    #[command(flatten)]
    NativeCommand(NativeCommand),
}

enum Args {
    NativeCommand(NativeCommand),
    GitCommand(Vec<String>),
}

impl RunExit for Args {
    fn run_exec(self) -> ! {
        match self {
            Args::NativeCommand(cmd) => cmd.run_exec(),
            Args::GitCommand(args_vec) => args_vec.run_exec(),
        }
    }
}

fn parse_args() -> Args {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec: Vec<String> = args_iter.collect();
    let args_vec2: Vec<String> = env::args().collect();
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "oaf" => {
            if args_vec2.len() > 1 {
                let x = Opt::try_parse_from(&args_vec2[0..2]);
                if let Err(e) = x {
                    if let clap::error::ErrorKind::UnknownArgument
                    | clap::error::ErrorKind::InvalidSubcommand = e.kind()
                    {
                        return Args::GitCommand(args_vec);
                    }
                }
            }
            Opt::parse()
        }
        _ => {
            let mut args = vec!["oaf".to_string()];
            args.push(match progname.split_once('-') {
                Some(("git", _)) => {
                    eprintln!("Unsupported command name {}", progname);
                    exit(1);
                }
                Some((_, cmd)) => cmd.to_owned(),
                _ => {
                    eprintln!("Unsupported command name {}", progname);
                    exit(1);
                }
            });
            args.extend(args_vec.into_iter());
            Opt::parse_from(args)
        }
    };
    match opt {
        Opt::NativeCommand(cmd) => Args::NativeCommand(cmd),
    }
}

fn main() {
    parse_args().run_exec();
}
