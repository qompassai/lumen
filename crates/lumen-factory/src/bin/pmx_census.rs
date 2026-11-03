//! pmx-census command-line entry point (pmx-import/1).
//!
//! Usage: `pmx_census <file.pmx>`
//!
//! Prints the `pmx-census/1` JSON record to stdout. Exit codes:
//! 0 = census written, 1 = parse or read failure (reason on
//! stderr), 2 = usage error. Input size is capped before reading
//! (see `lumen_factory::convert::pmx::parse::INPUT_BYTES_MAX`).

#![forbid(unsafe_code)]

use std::io::Read;
use std::process::ExitCode;

use lumen_factory::convert::pmx::census::census;
use lumen_factory::convert::pmx::parse::INPUT_BYTES_MAX;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: pmx_census <file.pmx>");
        return ExitCode::from(2);
    }
    let path = &args[1];

    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("pmx_census: cannot open {path}: {err}");
            return ExitCode::FAILURE;
        }
    };
    let cap = INPUT_BYTES_MAX as u64;
    let mut bytes = Vec::new();
    if let Err(err) = file.by_ref().take(cap + 1).read_to_end(&mut bytes) {
        eprintln!("pmx_census: cannot read {path}: {err}");
        return ExitCode::FAILURE;
    }

    match census(&bytes) {
        Ok(record) => {
            print!("{}", record.to_json());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("pmx_census: {path}: {err}");
            ExitCode::FAILURE
        }
    }
}
