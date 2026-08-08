//! `aviary-media` — a bounded, read-only MCP server for the media board.

use aviary_lib::{mcp_media::MediaServer, mcp_protocol};

fn main() {
    let collection = match arguments(std::env::args().skip(1)) {
        Ok(Arguments::Run { collection }) => collection,
        Ok(Arguments::Help) => {
            println!("Usage: aviary-media [--collection <positive-id>]");
            return;
        }
        Err(error) => {
            eprintln!("aviary-media: {error}");
            std::process::exit(2);
        }
    };
    let server = match MediaServer::current(collection) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("aviary-media: {error}");
            std::process::exit(2);
        }
    };
    if let Err(error) = mcp_protocol::serve_stdio(server) {
        eprintln!("aviary-media: {error}");
        std::process::exit(1);
    }
}

enum Arguments {
    Run { collection: Option<i64> },
    Help,
}

fn arguments(args: impl IntoIterator<Item = String>) -> Result<Arguments, String> {
    let mut args = args.into_iter();
    let mut collection = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(Arguments::Help),
            "--collection" if collection.is_none() => {
                let raw = args
                    .next()
                    .ok_or_else(|| "--collection requires an id".to_string())?;
                let id = raw
                    .parse::<i64>()
                    .ok()
                    .filter(|id| *id > 0)
                    .ok_or_else(|| "--collection id must be a positive integer".to_string())?;
                collection = Some(id);
            }
            "--collection" => return Err("--collection may only be supplied once".into()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(Arguments::Run { collection })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_argument_is_explicit_and_bounded() {
        assert!(matches!(
            arguments(["--collection".into(), "7".into()]).unwrap(),
            Arguments::Run {
                collection: Some(7)
            }
        ));
        assert!(arguments(["--collection".into(), "0".into()]).is_err());
        assert!(arguments(["--collection".into()]).is_err());
        assert!(arguments(["--unknown".into()]).is_err());
    }
}
