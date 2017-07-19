#![feature(custom_derive, plugin)] #![plugin(rocket_codegen)]
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

use std::collections::HashMap;
use std::io::{self, BufReader, Write};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;
use std::time::SystemTime;
use rocket::State;
use rocket::http::{Cookie, Cookies, RawStr, Status};
use rocket::http::uri::URI;
use rocket::outcome::IntoOutcome;
use rocket::request::{Form, Request, FromParam, FromRequest, FromFormValue, Outcome};
use rocket::response::{Failure, Redirect, NamedFile};
use rocket_contrib::Template;
use glob::glob;
use rand::Rng;
use rand::distributions::{IndependentSample, Range};
use flow::{Flow, FlowCmd};

error_chain! {
    errors {
        Parse(p: PathBuf) {}
        BadParam(msg: &'static str) {}
        Rocket(f: Failure) {}
    }

    foreign_links {
        Io(io::Error);
        Glob(glob::GlobError);
        GlobPattern(glob::PatternError);
    }
}
impl From<Failure> for Error {
    fn from(f: Failure) -> Self {
        Error::from_kind(ErrorKind::Rocket(f))
    }
}
use std::result::Result as StdResult;

const DATADIRS: &[&str] = &["/mnt/usbstick/proton_data", "/mnt/vertical/proton_data"];
const RATINGS: &str = "ratings.csv";
const REPORTS: &str = "reports.csv";

#[derive(Default)]
struct UserInfo {
    seen: Vec<(Datestamp, FlowType, u32)>,
    rate_error: bool,
    report_error: bool
}

type ActiveUsers = Mutex<HashMap<User, UserInfo>>;

fn main() {
    try_main().unwrap();
}

fn try_main() -> Result<()> {
    println!("Initializing output files...");
    let meta = fs::metadata(RATINGS);
    if meta.map(|m| m.len() == 0).unwrap_or(true) {
        let mut ratings_file = File::create(RATINGS)?;
        writeln!(&mut ratings_file, "User,Date,Flow type,Number,Warm,Hard,Rough,Sticky")?;
    }
    let meta = fs::metadata(REPORTS);
    if meta.map(|m| m.len() == 0).unwrap_or(true) {
        let mut reports_file = File::create(REPORTS)?;
        writeln!(&mut reports_file, "User,Date,Flow type,Number,Dark,Bright,Blurry,Grainy")?;
    }

    println!("Scanning surfaces...");
    let mut surfaces = vec![];
    for dir in DATADIRS {
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
        .mount("/", routes![index, get_file, list,
                            login_from_query, login_from_header, logged_in,
                            episode, episode_login,
                            random, random_login,
                            rate, rate_login, report, report_login,
                           ])
        .manage(surfaces)
        .manage(ActiveUsers::default())
        .attach(Template::fairing())
        .launch();

    Ok(())
}

#[derive(FromForm)]
struct Login {
    redir: String,
    user_name: String
}

#[derive(FromForm)]
struct Referer {
    uri: String
}

impl<'a, 'r> FromRequest<'a, 'r> for Referer {
    type Error = ();

    fn from_request(req: &'a Request<'r>) -> Outcome<Self, ()> {
        req.headers()
           .get_one("Referer")
           .map(|s| Referer { uri: s.into() })
           .or_forward(())
    }
}

#[get("/login?<refer>")]
fn login_from_query(refer: Referer) -> Template {
    Template::render("login", json!({ "redir": refer.uri }))
}

#[get("/login")]
fn login_from_header(refer: Referer) -> Template {
    Template::render("login", json!({ "redir": refer.uri }))
}

#[post("/logged_in", data="<login>")]
fn logged_in(mut cookies: Cookies, login: Form<Login>) -> Redirect {
    let login = login.get();
    cookies.add(Cookie::new("user", login.user_name.clone()));
    Redirect::to(&login.redir)
}

#[derive(Serialize)]
struct SurfaceData {
    date: Datestamp,
    flow: FlowType,
    num: u32,
    image: String,
    ratings: Vec<Rating>
}

impl SurfaceData {
    fn from_flow_file(date: Datestamp, flowname: FlowType, num: u32, path: &Path) -> Result<Self> {
        let flow = Flow::parse(format!("{}", flowname),
                               BufReader::new(File::open(path)?))
                       .chain_err(|| ErrorKind::Parse(path.to_owned()))?;
        Ok(Self {
            flow: flowname,
            date, num,
            image: path.with_file_name("surface.png").to_string_lossy().into_owned(),
            ratings: flow.states.iter()
                                .find(|s| s.name.starts_with("Wrap up")).unwrap() // TODO parse timestamps in flow crate
                                .script.iter()
                                       .filter_map(|&(ref cmd, _)| match *cmd {
                                           FlowCmd::Int { ref prompt, data: Some(d), .. } => Some(Rating { prompt: prompt.clone(), answer: d as u8, ..Default::default() }),
                                           _ => None
                                       })
                                       .collect()
        })
    }
}

#[derive(Serialize, Default)]
struct Rating {
    prompt: String,
    answer: u8,
    
    #[serde(skip_serializing_if="Option::is_none")]
    short: Option<&'static str>,
    #[serde(skip_serializing_if="Option::is_none")]
    long: Option<&'static str>,
}

macro_rules! handle {
    ($(#[$attr:meta])* fn $name:ident $params:tt -> $ret:ty { $($body:tt)* }) => {
        $(#[$attr])*
        fn $name $params -> StdResult<$ret, Failure> {
            do catch { $($body)* }.map_err(|err: Error| {
                println!("ERROR: {:?}", err);
                let mut code = 500;
                match *err.kind() {
                    ErrorKind::Io(ref ioerr) => match ioerr.kind() {
                        io::ErrorKind::NotFound => code = 404,
                        io::ErrorKind::PermissionDenied => code = 403,
                        _ => {}
                    },
                    ErrorKind::BadParam { .. } => code = 400,
                    _ => {}
                }
                Failure(Status::from_code(code).unwrap())
            })
        }
    }
}

macro_rules! handle_login {
    (#[$method:ident($($route:tt)*)] fn $name:ident/$name_login:ident($user:ident: $user_ty:ty, $($params:tt)*) -> $ret:ty { $($body:tt)* }) => {
        #[$method($($route)*, rank=1)]
        #[allow(unused_variables)]
        fn $name_login(uri: &URI, $($params)*) -> Redirect {
            Redirect::to(&format!("/login?uri={}", URI::percent_encode(uri.as_str())))
        }

        handle! {
            #[$method($($route)*)]
            fn $name($user: $user_ty, $($params)*) -> $ret { $($body)* }
        }
    }
}

#[get("/")]
fn index() -> &'static str {
    "It works!"
}

handle! {
    #[get("/<file..>", rank=2)]
    fn get_file(file: PathBuf) -> NamedFile {
        let mut path = PathBuf::from("/");
        path.push(file);

        let path = path.canonicalize()?;
        if DATADIRS.iter().any(|dir| path.starts_with(dir)) {
            Ok(NamedFile::open(&path)?)
        } else {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "path not in any datadir"))?
        }
    }
}

handle! {
    #[get("/list")]
    fn list(surfaces: State<Vec<SurfaceData>>) -> Template {
        let start = SystemTime::now();

        Ok(Template::render("list", json!({ "surfaces": &*surfaces, "time": elapsed(start) })))
    }
}

#[derive(Copy, Clone, PartialEq, Serialize)]
struct Datestamp(u32);
impl<'a> FromParam<'a> for Datestamp {
    type Error = String;
    fn from_param(param: &'a RawStr) -> StdResult<Self, Self::Error> {
        Ok(Datestamp(
            param.url_decode().map_err(|e| format!("bad UTF8 in date string at byte {}", e.valid_up_to()))?
                 .parse::<u32>().map_err(|_| format!("date string not an integer"))?))
    }
}
impl fmt::Display for Datestamp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Copy, Clone, PartialEq, Serialize)]
enum FlowType {
    StickCam,
    OptoCam,
    BioCam
}
impl<'a> FromParam<'a> for FlowType {
    type Error = &'static str;
    fn from_param(param: &'a RawStr) -> StdResult<Self, Self::Error> {
        param.as_str().parse()
    }
}
impl FromStr for FlowType {
    type Err = &'static str;

    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        match s {
            "stickcam" => Ok(FlowType::StickCam),
            "optocam" => Ok(FlowType::OptoCam),
            "biocam" => Ok(FlowType::BioCam),
            _ => Err("invalid flow type")
        }
    }
}
impl fmt::Display for FlowType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            FlowType::StickCam => write!(f, "stickcam"),
            FlowType::OptoCam => write!(f, "optocam"),
            FlowType::BioCam => write!(f, "biocam"),
        }
    }
}

#[derive(Serialize, Clone, Default, PartialEq, Eq, Hash)]
struct User {
    name: String
}

impl<'a, 'r> FromRequest<'a, 'r> for User {
    type Error = ();

    fn from_request(request: &'a Request<'r>) -> Outcome<User, ()> {
        request.cookies()
               .get("user")
               .map(|c| User { name: c.value().to_owned() })
               .or_forward(())
    }
}

handle_login! {
    #[get("/<date>/<flow>/<idx>")]
    fn episode/episode_login(user: User, users: State<ActiveUsers>, date: Datestamp, flow: Option<FlowType>, idx: u32) -> Template {
        let flow = flow.ok_or(ErrorKind::BadParam("invalid flow type"))?;

        let mut data = {
            let mut data = None;
            for dir in DATADIRS {
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
    fn random/random_login(user: User, users: State<ActiveUsers>, surfaces: State<Vec<SurfaceData>>) -> Template {

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

struct Likert(u8);

impl<'v> FromFormValue<'v> for Likert {
    type Error = &'v RawStr;

    fn from_form_value(v: &'v RawStr) -> StdResult<Self, Self::Error> {
        match v.as_str().parse() {
            Ok(n @ 1...5) => Ok(Likert(n)),
            _ => Err(v)
        }
    }
}

#[derive(FromForm)]
struct Ratings {
    image: String,
    warm: Likert,
    hard: Likert,
    rough: Likert,
    sticky: Likert
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

        let mut file = OpenOptions::new().append(true).open(RATINGS)?;
        writeln!(&mut file, "{},{},{},{},{},{},{},{}", user.name, date, flowname, num, ratings.warm.0, ratings.hard.0, ratings.rough.0, ratings.sticky.0)?;

        Ok(Redirect::to("/random"))
    }
}

#[derive(FromForm)]
struct Report {
    image: String,
    dark: bool,
    bright: bool,
    blurry: bool,
    grainy: bool
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

        let mut file = OpenOptions::new().append(true).open(REPORTS)?;
        writeln!(&mut file, "{},{},{},{},{},{},{},{}", user.name, date, flowname, num, report.dark, report.bright, report.blurry, report.grainy)?;

        Ok(Redirect::to("/random"))
    }
}

fn elapsed(start: SystemTime) -> String {
    let dur = SystemTime::now().duration_since(start).unwrap();
    format!("{}.{}s", dur.as_secs(), dur.subsec_nanos() / 1_000_000)
}

fn extract_path(path: &Path) -> (Datestamp, FlowType, u32) {
    macro_rules! x { ($e:expr) => { $e.and_then(|s| s.as_os_str().to_str()).and_then(|s| s.parse().ok()).unwrap() } }

    let mut comps = path.components().rev();
    let num = x!(comps.nth(1));
    let flowname = x!(comps.next());
    let date = Datestamp(x!(comps.next()));

    return (date, flowname, num);
}

