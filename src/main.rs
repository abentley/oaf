use std::env;
use std::path::PathBuf;
use std::process::{Command};
use std::os::unix::process::CommandExt;
use structopt::{StructOpt, clap};

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt{
    Cat {
        #[structopt(long, short, default_value="HEAD")]
        tree: String,
        input: String,
    }
}


fn make_git(command: &str) -> Command{
    let mut c = Command::new("git");
    c.arg(command);
    c
}


fn cat_cmd(input: &str, mut tree: &str) -> std::io::Error{
    if tree == "index"{
        tree = "";
    }
    make_git("show").arg(format!("{}:./{}", tree, input)).exec()
}


fn parse_args(progpath: &PathBuf, args_vec: &Vec<String>) -> Result<Opt, clap::Error>{
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    match progname{
        "nit" => {
            Opt::from_args_safe()
        },
        _ => {
            let mut args = vec!["nit".to_string()];
            let mut subcmd_iter = progname.split('-');
            subcmd_iter.next();
            for arg in subcmd_iter{
                args.push(arg.to_string());
            }
            for arg in args_vec{
                args.push(arg.to_string());
            }
            Ok(Opt::from_iter(args))
        }
    }
}


fn main() {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let args_vec:Vec<String> = args_iter.collect();
    let opt = parse_args(&progpath, &args_vec);
    match opt {
        Ok(Opt::Cat{input, tree}) => cat_cmd(&input, &tree),
        Err(err) => {
            if err.kind == clap::ErrorKind::UnknownArgument {
                let mut git_args = vec![];
                for git_arg in &args_vec{
                    git_args.push(git_arg.clone());
                }
                Command::new("git").args(git_args).exec()
            }
            else {
                err.exit();
            }
        },
    };
}
