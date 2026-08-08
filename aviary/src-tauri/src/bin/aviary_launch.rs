//! Private terminal-launch helper bundled beside the Aviary executable.

use aviary_lib::launch;
use std::ffi::OsString;

fn main() {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let descriptor: OsString = match (arguments.next(), arguments.next()) {
        (Some(descriptor), None) => descriptor,
        _ => {
            eprintln!("Aviary could not start this terminal session: invalid helper invocation");
            std::process::exit(2);
        }
    };
    match launch::execute_descriptor(&descriptor) {
        Ok(outcome) => std::process::exit(outcome.process_exit_code()),
        Err(error) => {
            eprintln!("Aviary could not start this terminal session: {error}");
            std::process::exit(1);
        }
    }
}
