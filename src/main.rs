// Copyright 2021 Aaron Bentley <aaron@aaronbentley.com>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
#![cfg_attr(feature = "strict", deny(warnings))]
use std::env;
use std::fmt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use structopt::{clap, StructOpt};

mod git;
pub use git::*;
mod worktree;
pub use worktree::*;
mod commands;
pub use commands::*;

#[derive(Debug)]
pub enum CommitErr {
    NoCommit { spec: String },
}

impl fmt::Display for CommitErr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CommitErr::NoCommit { spec } => write!(f, "No commit found for \"{}\"", spec),
        }
    }
}

impl FromStr for Commit {
    type Err = CommitErr;
    fn from_str(spec: &str) -> std::result::Result<Self, <Self as FromStr>::Err> {
        match eval_rev_spec(spec) {
            Err(..) => Err(CommitErr::NoCommit {
                spec: spec.to_string(),
            }),
            Ok(sha) => Ok(Commit { sha }),
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt {
    #[structopt(flatten)]
    NativeCommand(NativeCommand),
    #[structopt(flatten)]
    RewriteCommand(RewriteCommand),
}

enum Args {
    NativeCommand(NativeCommand),
    GitCommand(Vec<String>),
}

fn parse_args() -> Args {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec: Vec<String> = args_iter.collect();
    let args_vec2: Vec<String> = env::args().collect();
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "nit" => {
            if args_vec2.len() > 1 {
                let x = Opt::from_iter_safe(&args_vec2[0..2]);
                if let Err(err) = x {
                    if err.kind == clap::ErrorKind::UnknownArgument {
                        return Args::GitCommand(args_vec);
                    }
                    if err.kind == clap::ErrorKind::InvalidSubcommand {
                        return Args::GitCommand(args_vec);
                    }
                }
            }
            Opt::from_args()
        }
        _ => {
            let mut args = vec!["nit".to_string()];
            let mut subcmd_iter = progname.split('-');
            subcmd_iter.next();
            for arg in subcmd_iter {
                args.push(arg.to_string());
            }
            for arg in &args_vec {
                args.push(arg.to_string());
            }
            Opt::from_iter(args)
        }
    };
    match opt {
        Opt::RewriteCommand(cmd) => Args::GitCommand(match cmd.make_args() {
            Ok(args) => args,
            Err(status) => {
                exit(status);
            }
        }),
        Opt::NativeCommand(cmd) => Args::NativeCommand(cmd),
    }
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::NativeCommand(cmd) => exit(cmd.run()),
        Args::GitCommand(args_vec) => {
            make_git_command(&args_vec).exec();
        }
    };
}
