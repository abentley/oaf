use std::process::{Command};
use std::os::unix::process::CommandExt;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt()]
enum Opt{
    Cat {
        input: String,
    }
}


fn make_git(command: &str) -> Command{
    let mut c = Command::new("git");
    c.arg(command);
    c
}


fn cat_cmd(input: &str) -> std::io::Error{
    make_git("show").arg(format!(":{}", input)).exec()
}


fn main() {
    let opt = Opt::from_args();
    let result = match opt {
        Opt::Cat{input} => cat_cmd(&input)
    };
    println!("{:?}", result);
}
