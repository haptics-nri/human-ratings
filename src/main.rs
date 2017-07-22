#![feature(custom_derive)]
#![feature(macro_vis_matcher)]
#![feature(catch_expr)]
#![feature(never_type)]
#![feature(stmt_expr_attributes)]
#![feature(plugin)]
#![plugin(rocket_codegen)]
#![allow(unreachable_patterns)] // TODO fix rocket bug re infallible FromParam

extern crate rocket;
extern crate rocket_contrib;
extern crate serde;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serde_json;
extern crate csv;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate iflet;
#[macro_use] extern crate unborrow;
extern crate glob;
extern crate rand;
extern crate flow;

#[macro_use] mod macros;
mod errors;
mod routes;
mod settings;
mod structs;
mod utils;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use rocket_contrib::Template;
use glob::glob;

// TODO remove globs
use errors::*;
use structs::*;
use utils::*;

fn main() {
    try_main().unwrap();
    unreachable!();
}

fn try_main() -> Result<!> {
    println!("Initializing output files...");
    let mut users: HashMap<User, UserInfo> = HashMap::new();
    output_file(settings::RATINGS,
                |mut csv| {
                    let ratings = csv.headers()?.iter()
                                                .skip(4)
                                                .map(|s| s.to_lowercase())
                                                .collect::<Vec<_>>();
                    for row in csv.records() {
                        let mut row = row?;
                        let answers = row.iter()
                                         .skip(4)
                                         .map(|s| s.parse())
                                         .collect::<StdResult<Vec<_>,_>>()?;
                        row.truncate(4);
                        let row: SurfaceDataWithUser = row.deserialize(None)?;
                        let (mut surface, username) = row.without_user();
                        surface.ratings = ratings.iter()
                                                 .cloned()
                                                 .zip(answers)
                                                 .collect();
                        let user_info = users.entry(User { name: username }).or_insert_with(Default::default);
                        user_info.seen.push((surface.date, surface.flow, surface.num));
                    }
                    Ok(())
                })?;
    output_file(settings::REPORTS,
                |mut csv| {
                    unborrow!(csv.set_headers(csv.headers().unwrap()
                                                 .iter()
                                                 .map(|s| s.split(' ').next().unwrap().to_lowercase())
                                                 .collect()));
                    for row in csv.deserialize() {
                        let row: ReportWithUser = row?;
                        let (report, username) = row.without_user();
                        let user_info = users.entry(User { name: username }).or_insert_with(Default::default);
                        user_info.seen.push((report.date, report.flow, report.num));
                    }
                    Ok(())
                })?;

    println!("Scanning surfaces...");
    let mut surfaces = vec![];
    for dir in settings::DATADIRS {
        for path in glob(&format!("{}/*/biocam/*/surface.png", dir))? {
            let path = path?;
            let (date, flowname, num) = extract_path(&path);
            surfaces.push(SurfaceData::from_flow_file(date, flowname, num,
                                                      &path.with_file_name(format!("{}", flowname))
                                                           .with_extension("flow"))?);
        }
    }

    println!("Launching rocket...");
    Err(rocket::ignite()
        .mount("/", routes![routes::index, routes::get_file, routes::list,
                            routes::login_from_query, routes::login_from_header, routes::logged_in,
                            routes::episode, routes::episode_login,
                            routes::random, routes::random_login,
                            routes::rate, routes::rate_login, routes::report, routes::report_login,
                           ])
        .manage(surfaces)
        .manage(Mutex::new(users))
        .attach(Template::fairing())
        .launch())?;

    unreachable!();
}

pub fn output_file<P: AsRef<Path>, F: FnOnce(csv::Reader<File>) -> Result<()>>(p: P, process: F) -> Result<()> {
    println!("\treading file {:?}", p.as_ref());

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(p)?;

    file.seek(SeekFrom::Start(0))?;
    let csv = csv::Reader::from_reader(file);
    process(csv)
}

