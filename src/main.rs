use std::env;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use structopt::{clap, StructOpt};

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt {
    Cat {
        #[structopt(long, short, default_value = "HEAD")]
        tree: String,
        input: String,
    },
}

fn cat_args(input: &str, mut tree: &str) -> Vec<String> {
    if tree == "index" {
        tree = "";
    }
    vec!["show".to_string(), format!("{}:./{}", tree, input)]
}

enum Args {
    //NativeCommand(Opt),
    GitCommand(Vec<String>),
}

fn parse_args() -> Args {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec: Vec<String> = args_iter.collect();
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname {
        "nit" => Opt::from_args_safe(),
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
            Ok(Opt::from_iter(args))
        }
    };
    match opt {
        Ok(Opt::Cat { input, tree }) => Args::GitCommand(cat_args(&input, &tree)),
        Err(err) => {
            if err.kind != clap::ErrorKind::UnknownArgument {
                err.exit();
            }
            Args::GitCommand(args_vec)
        }
    }
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::GitCommand(args_vec) => Command::new("git").args(args_vec).exec(),
    };
}
