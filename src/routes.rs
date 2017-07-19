use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rocket::State;
use rocket::http::{Cookie, Cookies, Status};
use rocket::http::uri::URI;
use rocket::request::Form;
use rocket::response::{Failure, Redirect, NamedFile};
use rocket_contrib::Template;
use rand::distributions::{IndependentSample, Range};

use rand;
use settings;
use errors::*;
use structs::*;
use utils::*;

#[get("/login?<refer>")]
pub fn login_from_query(refer: Referer) -> Template {
    Template::render("login", json!({ "redir": refer.uri }))
}

#[get("/login")]
pub fn login_from_header(refer: Referer) -> Template {
    Template::render("login", json!({ "redir": refer.uri }))
}

#[post("/logged_in", data="<login>")]
pub fn logged_in(mut cookies: Cookies, login: Form<Login>) -> Redirect {
    let login = login.get();
    cookies.add(Cookie::new("user", login.user_name.clone()));
    Redirect::to(&login.redir)
}

#[get("/")]
pub fn index() -> &'static str {
    "It works!"
}

handle! {
    #[get("/<file..>", rank=2)]
    pub fn get_file(file: PathBuf) -> NamedFile {
        let mut path = PathBuf::from("/");
        path.push(file);

        let path = path.canonicalize()?;
        if settings::DATADIRS.iter().any(|dir| path.starts_with(dir)) {
            Ok(NamedFile::open(&path)?)
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "path not in any datadir"))?
        }
    }
}

handle! {
    #[get("/list")]
    pub fn list(surfaces: State<Vec<SurfaceData>>) -> Template {
        let start = SystemTime::now();

        Ok(Template::render("list", json!({ "surfaces": &*surfaces, "time": elapsed(start) })))
    }
}

handle_login! {
    #[get("/<date>/<flow>/<idx>")]
    pub fn episode/episode_login(user: User, users: State<ActiveUsers>, date: Datestamp, flow: Option<FlowType>, idx: u32) -> Template {
        let flow = flow.ok_or(ErrorKind::BadParam("invalid flow type"))?;

        let mut data = {
            let mut data = None;
            for dir in settings::DATADIRS {
                let mut path = PathBuf::from(dir);
                path.push(format!("{}", date.0));
                path.push(format!("{}", flow));
                path.push(format!("{}", idx));
                path.push(format!("{}.flow", flow));

                data = match SurfaceData::from_flow_file(date, flow, idx, &path) {
                    Ok(d) => Some(d),
                    Err(e) => match *e.kind() {
                        ErrorKind::Io(ref ioe) => match ioe.kind() {
                            io::ErrorKind::NotFound => continue,
                            _ => break
                        },
                        _ => break
                    }
                };
            }

            data.ok_or(io::Error::new(io::ErrorKind::NotFound, "episode not found in any datadir"))?
        };

        for rating in data.ratings.iter_mut() {
            if rating.prompt == "cool/warm" {
                rating.short = Some("warm");
                rating.long = Some("What temperature would you feel when touching this surface? 1: ice cold beer bottle. 5: hot sand at the beach.");
            } else if rating.prompt == "soft/hard" {
                rating.short = Some("hard");
                rating.long = Some("How soft or hard is this surface? 1: pillow. 5: rock.");
            } else if rating.prompt == "smooth/rough" {
                rating.short = Some("rough");
                rating.long = Some("How smooth or rough is this surface? 1: glass. 5: sandpaper.");
            } else if rating.prompt == "slippery/sticky" {
                rating.short = Some("sticky");
                rating.long = Some("How slippery or sticky is this surface? This is NOT the same as roughness, nor is it sticky as in glue. This question refers to how much a finger would get stuck while rubbing due to friction with the surface. 1: silk. 5: rubber.");
            }
        }

        let mut users = users.lock().unwrap();
        let user_info = users.entry(user.clone()).or_insert_with(Default::default);
        let rate_error = if user_info.rate_error {
            user_info.rate_error = false;
            "All ratings are required"
        } else { "" };
        let report_error = if user_info.report_error {
            user_info.report_error = false;
            "At least one reason is required"
        } else { "" };

        Ok(Template::render("episode",
                            json!({
                                "rate_error": rate_error,
                                "report_error": report_error,
                                "user": user,
                                "surface": data,
                                "date": date.0,
                                "flow": flow.to_string(),
                                "idx": idx
                            })))
    }
}

handle_login! {
    #[get("/random")]
    pub fn random/random_login(user: User, users: State<ActiveUsers>, surfaces: State<Vec<SurfaceData>>) -> Template {

        let mut rng = rand::thread_rng();
        let range = Range::new(0, surfaces.len());
        let (mut date, mut flow, mut num);
        {
            let mut users = users.lock().unwrap();
            let user_info = users.entry(user.clone()).or_insert_with(Default::default);
            loop {
                let idx = range.ind_sample(&mut rng);
                date = surfaces[idx].date;
                flow = surfaces[idx].flow;
                num = surfaces[idx].num;
                if !user_info.seen.contains(&(date, flow, num)) {
                    break;
                }
            }
        }

        Ok(episode(user, users, date, Some(flow), num)?)
    }
}

handle_login! {
    #[post("/rate", data="<ratings>")]
    fn rate/rate_login(user: User, referer: Referer, users: State<ActiveUsers>, ratings: Option<Form<Ratings>>) -> Redirect {
        let mut users = users.lock().unwrap();
        let user_info = users.entry(user.clone()).or_insert_with(Default::default);

        guard!(let Some(ratings) = ratings else {
            user_info.rate_error = true;
            return Ok(Redirect::to(&referer.uri))
        });
        let ratings = ratings.get();

        let (date, flowname, num) = extract_path(Path::new(&ratings.image));
        user_info.seen.push((date, flowname, num));

        let mut file = OpenOptions::new().append(true).open(settings::RATINGS)?;
        writeln!(&mut file, "{},{},{},{},{},{},{},{}", user.name, date, flowname, num, ratings.warm.0, ratings.hard.0, ratings.rough.0, ratings.sticky.0)?;

        Ok(Redirect::to("/random"))
    }
}

handle_login! {
    #[post("/report", data="<report>")]
    fn report/report_login(user: User, referer: Referer, users: State<ActiveUsers>, report: Form<Report>) -> Redirect {
        let mut users = users.lock().unwrap();
        let user_info = users.entry(user.clone()).or_insert_with(Default::default);
        let report = report.get();

        if !(report.dark || report.bright || report.blurry || report.grainy) {
            user_info.report_error = true;
            return Ok(Redirect::to(&referer.uri))
        }

        let (date, flowname, num) = extract_path(Path::new(&report.image));
        user_info.seen.push((date, flowname, num));

        let mut file = OpenOptions::new().append(true).open(settings::REPORTS)?;
        writeln!(&mut file, "{},{},{},{},{},{},{},{}", user.name, date, flowname, num, report.dark, report.bright, report.blurry, report.grainy)?;

        Ok(Redirect::to("/random"))
    }
}


