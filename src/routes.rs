use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
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
    #[get("/image/<date>/<flow>/<idx>")]
    pub fn get_file(date: Datestamp, flow: FlowType, idx: u32) -> NamedFile {
        for dir in settings::DATADIRS {
            let mut path = PathBuf::from(dir);
            path.push(format!("{}", date));
            path.push(format!("{}", flow));
            path.push(format!("{}", idx));
            path.push("surface.png");

            match NamedFile::open(&path) {
                Ok(file) => return Ok(file),
                Err(ref e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => Err(ErrorKind::IoOp(e, "open", path.to_owned()))?
            }
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "image not found in any datadir").into())
    }
}

handle! {
    #[get("/list")]
    pub fn list(surfaces: State<Vec<SurfaceData>>, reports: State<Reports>) -> Template {
        let start = SystemTime::now();

        let reports = reports.lock().unwrap();
        let surfaces = surfaces.iter()
            .map(|surf| {
                let mut json = ::serde_json::to_value(surf).unwrap();
                if reports.contains(&(surf.date, surf.flow, surf.num)) {
                    json.as_object_mut().unwrap().insert("report".into(), true.into());
                }
                json
            })
            .collect::<Vec<_>>();

        Ok(Template::render("list", json!({
            "surfaces": surfaces,
            "time": elapsed(start)
        })))
    }
}

handle_login! {
    #[get("/<date>/<flow>/<idx>")]
    pub fn episode/episode_login(user: User, users: State<ActiveUsers>, date: Datestamp, flow: Option<FlowType>, idx: u32) -> Template {
        let flow = flow.ok_or(ErrorKind::BadParam("invalid flow type"))?;

        let data = {
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
    #[post("/rate", data="<form>")]
    fn rate/rate_login(user: User, users: State<ActiveUsers>, surfaces: State<Vec<SurfaceData>>, form: Form<SurfaceData>) -> Template {
        let SurfaceData { date, flow, num, ratings } = form.into_inner();
        if_chain!([let Some(warm) => ratings.get("warm"),
                   let Some(hard) => ratings.get("hard"),
                   let Some(rough) => ratings.get("rough"),
                   let Some(sticky) => ratings.get("sticky")] {
            {
                let mut users = users.lock().unwrap();
                let user_info = users.entry(user.clone()).or_insert_with(Default::default);
                user_info.seen.push((date, flow, num));
            }

            let mut file = OpenOptions::new().append(true).open(settings::RATINGS)?;
            writeln!(&mut file, "{},{},{},{},{},{},{},{}", user.name, date, flow, num, warm.0, hard.0, rough.0, sticky.0)?;

            Ok(random(user, users, surfaces)?)
        } else {
            {
                let mut users = users.lock().unwrap();
                let user_info = users.entry(user.clone()).or_insert_with(Default::default);
                user_info.rate_error = true;
            }
            Ok(episode(user, users, date, Some(flow), num)?)
        })
    }
}

handle_login! {
    #[post("/report", data="<report>")]
    fn report/report_login(user: User, users: State<ActiveUsers>, surfaces: State<Vec<SurfaceData>>, reports: State<Reports>, report: Form<Report>) -> Template {
        let Report { date, flow, num, dark, bright, blurry, grainy } = report.into_inner();

        if dark || bright || blurry || grainy {
            {
                let mut users = users.lock().unwrap();
                let user_info = users.entry(user.clone()).or_insert_with(Default::default);
                user_info.seen.push((date, flow, num));
            }

            reports.lock().unwrap().insert((date, flow, num));

            let mut file = OpenOptions::new().append(true).open(settings::REPORTS)?;
            writeln!(&mut file, "{},{},{},{},{},{},{},{}", user.name, date, flow, num, dark, bright, blurry, grainy)?;

            Ok(random(user, users, surfaces)?)
        } else {
            {
                let mut users = users.lock().unwrap();
                let user_info = users.entry(user.clone()).or_insert_with(Default::default);
                user_info.report_error = true;
            }
            Ok(episode(user, users, date, Some(flow), num)?)
        }

    }
}


