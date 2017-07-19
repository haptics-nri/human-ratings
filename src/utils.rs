use std::path::Path;
use std::time::SystemTime;

use structs::{Datestamp, FlowType};

pub fn elapsed(start: SystemTime) -> String {
    let dur = SystemTime::now().duration_since(start).unwrap();
    format!("{}.{}s", dur.as_secs(), dur.subsec_nanos() / 1_000_000)
}

pub fn extract_path(path: &Path) -> (Datestamp, FlowType, u32) {
    macro_rules! x { ($e:expr) => { $e.and_then(|s| s.as_os_str().to_str()).and_then(|s| s.parse().ok()).unwrap() } }

    let mut comps = path.components().rev();
    let num = x!(comps.nth(1));
    let flowname = x!(comps.next());
    let date = Datestamp(x!(comps.next()));

    return (date, flowname, num);
}

