#![feature(custom_derive, plugin)] #![plugin(rocket_codegen)]
#![feature(macro_vis_matcher)]
#![feature(catch_expr)]

extern crate rocket;
extern crate rocket_contrib;
#[macro_use] extern crate serde_derive;
#[macro_use] extern crate serde_json;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate guard;
extern crate glob;
extern crate rand;
extern crate flow;

#[macro_use] mod macros;
mod errors;
mod routes;
mod settings;
mod structs;
mod utils;

use std::fs::{self, File};
use std::io::Write;
use rocket_contrib::Template;
use glob::glob;

// TODO remove globs
use errors::*;
use structs::*;
use utils::*;

fn main() {
    try_main().unwrap();
}

fn try_main() -> Result<()> {
    println!("Initializing output files...");
    let meta = fs::metadata(settings::RATINGS);
    if meta.map(|m| m.len() == 0).unwrap_or(true) {
        let mut ratings_file = File::create(settings::RATINGS)?;
        writeln!(&mut ratings_file, "User,Date,Flow type,Number,Warm,Hard,Rough,Sticky")?;
    }
    let meta = fs::metadata(settings::REPORTS);
    if meta.map(|m| m.len() == 0).unwrap_or(true) {
        let mut reports_file = File::create(settings::REPORTS)?;
        writeln!(&mut reports_file, "User,Date,Flow type,Number,Dark,Bright,Blurry,Grainy")?;
    }

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
    rocket::ignite()
        .mount("/", routes![routes::index, routes::get_file, routes::list,
                            routes::login_from_query, routes::login_from_header, routes::logged_in,
                            routes::episode, routes::episode_login,
                            routes::random, routes::random_login,
                            routes::rate, routes::rate_login, routes::report, routes::report_login,
                           ])
        .manage(surfaces)
        .manage(ActiveUsers::default())
        .attach(Template::fairing())
        .launch();

    Ok(())
}

