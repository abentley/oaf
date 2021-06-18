use std::env;
use std::path::PathBuf;
use std::process::{Command};
use std::os::unix::process::CommandExt;
use structopt::StructOpt;

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


fn main() {
    let mut args_iter = env::args();
    let progpath = PathBuf::from(args_iter.next().unwrap());
    let progname = progpath.file_name().unwrap().to_str().unwrap();
    let opt = match progname{
        "nit" => Opt::from_args(),
        _ => {
            let mut args = vec!["nit".to_string()];
            let mut subcmd_iter = progname.split('-');
            subcmd_iter.next();
            for arg in subcmd_iter{
                args.push(arg.to_string());
            }
            for arg in args_iter{
                args.push(arg);
            }
            Opt::from_iter(args)
        }
    };
    let result = match opt {
        Opt::Cat{input, tree} => cat_cmd(&input, &tree),
    };
    println!("{:?}", result);
}
