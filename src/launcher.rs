//! The program of Razor.app, the macOS service's first process: it replaces itself with the
//! command its arguments name. macOS charges a permission check to the program launchd started,
//! even after it replaces itself, so every agent the server runs asks as Razor, and a permission
//! granted to Razor outlives updates, since this program never changes with them.

use std::os::unix::process::CommandExt;
use std::path::Path;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let program = args.next().unwrap_or_default();
    let error = std::process::Command::new(&program).args(args).exec();
    eprintln!(
        "Razor.app cannot run {}: {error}.",
        Path::new(&program).display()
    );
    std::process::exit(1);
}
