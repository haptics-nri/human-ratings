use std::collections::HashMap;
use std::io::BufReader;
use std::fmt;
use std::fs::File;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

use rocket::http::RawStr;
use rocket::request::{Request, FromRequest, FromParam, FromFormValue, Outcome};
use rocket::outcome::IntoOutcome;
use flow::{Flow, FlowCmd};

use errors::*;

/// User ID (stored in a cookie and used to index into active users table)
#[derive(Serialize, Clone, Default, PartialEq, Eq, Hash)]
pub struct User {
    /// Name chosen by user at login screen
    pub name: String
}

/// Server-side information about a user
#[derive(Default)]
pub struct UserInfo {
    /// Surfaces already rated or reported by this user
    pub seen: Vec<(Datestamp, FlowType, u32)>,
    /// Flash message for rating form
    pub rate_error: bool, // TODO use FlashMessage
    /// Flash message for report form
    pub report_error: bool // TODO use FlashMessage
}

/// Managed state type for active users table
pub type ActiveUsers = Mutex<HashMap<User, UserInfo>>;

/// 1-5 rating of a surface property
pub struct Likert(pub u8);

/// Ratings that a human should give for a surface
#[derive(FromForm)]
pub struct Ratings {
    /// Full path to surface.png
    pub image: String,
    /// Rating for cold/warm
    pub warm: Likert,
    /// Rating for soft/hard
    pub hard: Likert,
    /// Rating for smooth/rough
    pub rough: Likert,
    /// Rating for slippery/sticky
    pub sticky: Likert
}

/// Report information for a bad image
#[derive(FromForm)]
pub struct Report {
    /// Full path to surface.png
    pub image: String,
    /// Is the image too dark?
    pub dark: bool,
    /// Is the image too bright?
    pub bright: bool,
    /// Is the image too blurry?
    pub blurry: bool,
    /// Is the image too grainy?
    pub grainy: bool
}

/// Surface info for passing to a template
#[derive(Serialize)]
pub struct SurfaceData {
    /// Episode date (e.g. $DATADIR/$date/$flow/$num)
    pub date: Datestamp,
    /// Episode flow type (e.g. $DATADIR/$date/$flow/$num)
    pub flow: FlowType,
    /// Episode number (e.g. $DATADIR/$date/$flow/$num)
    pub num: u32,
    /// Full path to surface.png
    pub image: String,
    /// Ratings loaded from flow file
    pub ratings: Vec<Rating>
}

/// Rating loaded from flow file
#[derive(Serialize, Default)]
pub struct Rating {
    /// Question shown during episode
    pub prompt: String,
    /// Answer given by experimenter
    pub answer: u8,
    
    /// Short version of prompt (used for radio button name)
    #[serde(skip_serializing_if="Option::is_none")]
    pub short: Option<&'static str>,
    /// Long version of prompt (displayed in rating form)
    #[serde(skip_serializing_if="Option::is_none")]
    pub long: Option<&'static str>,
}

/// YYYYMMDD date
#[derive(Copy, Clone, PartialEq, Serialize)]
pub struct Datestamp(pub u32);

/// Episode type (end-effector type)
#[derive(Copy, Clone, PartialEq, Serialize)]
pub enum FlowType {
    /// Rigid stick
    StickCam,
    /// OptoForce
    OptoCam,
    /// BioTac
    BioCam
}

/// Inputs from login form
#[derive(FromForm)]
pub struct Login {
    /// URL to redirect back to after logging in
    pub redir: String,
    /// Chosen username
    pub user_name: String
}

/// Passing the referer as a query param
#[derive(FromForm)]
pub struct Referer {
    /// Referring URL
    pub uri: String
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



impl<'a, 'r> FromRequest<'a, 'r> for User {
    type Error = ();

    fn from_request(request: &'a Request<'r>) -> Outcome<User, ()> {
        request.cookies()
               .get("user")
               .map(|c| User { name: c.value().to_owned() })
               .or_forward(())
    }
}

impl<'v> FromFormValue<'v> for Likert {
    type Error = &'v RawStr;

    fn from_form_value(v: &'v RawStr) -> StdResult<Self, Self::Error> {
        match v.as_str().parse() {
            Ok(n @ 1...5) => Ok(Likert(n)),
            _ => Err(v)
        }
    }
}

impl SurfaceData {
    pub fn from_flow_file(date: Datestamp, flowname: FlowType, num: u32, path: &Path) -> Result<Self> {
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

