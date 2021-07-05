use std::env;
use std::ffi::OsStr;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::str::from_utf8;
use structopt::{clap, StructOpt};

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt {
    Cat {
        #[structopt(long, short, default_value = "HEAD")]
        tree: String,
        input: String,
    },
    Push,
}

fn cat_args(input: &str, mut tree: &str) -> Vec<String> {
    if tree == "index" {
        tree = "";
    }
    vec!["show".to_string(), format!("{}:./{}", tree, input)]
}

fn get_current_branch() -> String {
    let stdout = make_git_command(vec!["branch", "--show-current"])
        .output()
        .expect("Could not determine branch.")
        .stdout;
    from_utf8(&stdout)
        .expect("Branch is not utf-8")
        .trim()
        .to_string()
}

fn branch_setting(branch: &str, setting: &str) -> String {
    format!("branch.{}.{}", branch, setting)
}

fn setting_exists(setting: &str) -> bool {
    let mut branch_remote_cmd = make_git_command(vec!["config", "--get", setting]);
    let status = branch_remote_cmd
        .output()
        .expect("Could not determine branch.")
        .status;
    status.success()
}

fn cmd_push() {
    let branch = get_current_branch();
    if setting_exists(&branch_setting(&branch, "remote")) {
        if !setting_exists(&branch_setting(&branch, "merge")) {
            panic!("Branch in unsupported state");
        }
        make_git_command(vec!["push"]).exec();
    } else {
        make_git_command(vec!["push", "-u", "origin", "HEAD"]).exec();
    }
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
    match opt {
        Ok(Opt::Cat { input, tree }) => Args::GitCommand(cat_args(&input, &tree)),
        Ok(opt) => Args::NativeCommand(opt),
        Err(err) => {
            if err.kind != clap::ErrorKind::UnknownArgument {
                err.exit();
            }
            Args::GitCommand(args_vec)
        }
    }
}

fn make_git_command<T: AsRef<OsStr>>(args_vec: Vec<T>) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args_vec);
    cmd
}

fn main() {
    let opt = parse_args();
    match opt {
        Args::NativeCommand(Opt::Push) => cmd_push(),
        // Not implemented here.
        Args::NativeCommand(Opt::Cat { .. }) => (),
        Args::GitCommand(args_vec) => {
            make_git_command(args_vec).exec();
        }
    };
}
