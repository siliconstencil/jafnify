//! Small root binary for the password-free `org.jafnify.route` polkit action.
//!
//! Exactly two verbs: `route-fallback` removes a dead tunnel's routing so the
//! laptop's normal internet works, and `route-restore` hands it back when the
//! tunnel returns. The fallback refuses while the kill switch is on. Nothing
//! else is accepted, and no user input is used.

use jafnify_linux::route::{self, RouteCommand};

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("jafnify-route: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match route::parse(&args)? {
        RouteCommand::Fallback => route::run_fallback(),
        RouteCommand::Restore => route::run_restore(),
    }
}
