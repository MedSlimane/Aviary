//! `aviary-library` — bounded, read-only MCP retrieval for Aviary's library.

use aviary_lib::{mcp_library::LibraryServer, mcp_protocol};

fn main() {
    match arguments(std::env::args().skip(1)) {
        Ok(Arguments::Run) => {}
        Ok(Arguments::Help) => {
            println!("Usage: aviary-library");
            return;
        }
        Err(error) => {
            eprintln!("aviary-library: {error}");
            std::process::exit(2);
        }
    }
    let server = match LibraryServer::current() {
        Ok(server) => server,
        Err(error) => {
            eprintln!("aviary-library: {error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = mcp_protocol::serve_stdio(server) {
        eprintln!("aviary-library: {error}");
        std::process::exit(1);
    }
}

enum Arguments {
    Run,
    Help,
}

fn arguments(args: impl IntoIterator<Item = String>) -> Result<Arguments, String> {
    let mut args = args.into_iter();
    match (args.next(), args.next()) {
        (None, None) => Ok(Arguments::Run),
        (Some(flag), None) if flag == "-h" || flag == "--help" => Ok(Arguments::Help),
        (Some(argument), _) => Err(format!("unknown argument: {argument}")),
        (None, Some(_)) => unreachable!("a second argument cannot exist without a first"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_help_or_no_arguments() {
        assert!(matches!(arguments([]).unwrap(), Arguments::Run));
        assert!(matches!(
            arguments(["--help".to_string()]).unwrap(),
            Arguments::Help
        ));
        assert!(arguments(["--path".to_string(), "/tmp".to_string()]).is_err());
    }
}
