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

fn make_git(command: &str) -> Command {
    let mut c = Command::new("git");
    c.arg(command);
    c
}

fn cat_cmd(input: &str, mut tree: &str) -> std::io::Error {
    if tree == "index" {
        tree = "";
    }
    make_git("show").arg(format!("{}:./{}", tree, input)).exec()
}

enum Args {
    NativeCommand(Opt),
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
    return match opt {
        Ok(opt) => Args::NativeCommand(opt),
        Err(err) => {
            if err.kind != clap::ErrorKind::UnknownArgument {
                err.exit();
            }
            Args::GitCommand(args_vec)
        }
    };
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::NativeCommand(Opt::Cat { input, tree }) => cat_cmd(&input, &tree),
        Args::GitCommand(args_vec) => Command::new("git").args(args_vec).exec(),
    };
}
