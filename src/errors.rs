use std::io;
use std::path::PathBuf;

use rocket::error::LaunchError;
use rocket::response::Failure;
use glob::{GlobError, PatternError};

pub use std::result::Result as StdResult;

error_chain! {
    errors {
        IoOp(ioerr: io::Error, op: &'static str, path: PathBuf) {}
        Parse(p: PathBuf) {}
        BadParam(msg: &'static str) {}
        Rocket(f: Failure) {}
    }

    foreign_links {
        Io(io::Error);
        Glob(GlobError);
        GlobPattern(PatternError);
        Launch(LaunchError);
        Csv(::csv::Error);
    }
}

// HACK: this makes it possible to invoke one handler from another
impl From<Failure> for Error {
    fn from(f: Failure) -> Self {
        Error::from_kind(ErrorKind::Rocket(f))
    }
}

