use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::fmt;
use std::fs::File;
use std::num::ParseIntError;
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

use rocket;
use rocket::http::RawStr;
use rocket::request::{Request, FromRequest, FromParam, FromForm, FromFormValue, FormItems, Outcome};
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
/// Managed state type for tracking reported bad surfaces
pub type Reports = Mutex<HashSet<(Datestamp, FlowType, u32)>>;

/// 1-5 rating of a surface property
#[derive(Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct Likert(pub u8);

with_user! {
    /// Report information for a bad image
    #[derive(FromForm, Serialize, Deserialize)]
    pub struct Report/ReportWithUser<String> {
        /// Episode date (e.g. $DATADIR/$date/$flow/$num)
        pub date: Datestamp,
        /// Episode flow type (e.g. $DATADIR/$date/$flow/$num)
        pub flow: FlowType,
        /// Episode number (e.g. $DATADIR/$date/$flow/$num)
        #[serde(rename="number")]
        pub num: u32,
        /// Is the image too dark?
        pub dark: bool,
        /// Is the image too bright?
        pub bright: bool,
        /// Is the image too blurry?
        pub blurry: bool,
        /// Is the image too grainy?
        pub grainy: bool
    }
}

with_user! {
    /// Surface info for passing to a template
    #[derive(Serialize, Deserialize)]
    pub struct SurfaceData/SurfaceDataWithUser<String> {
        /// Episode date (e.g. $DATADIR/$date/$flow/$num)
        pub date: Datestamp,
        /// Episode flow type (e.g. $DATADIR/$date/$flow/$num)
        pub flow: FlowType,
        /// Episode number (e.g. $DATADIR/$date/$flow/$num)
        #[serde(rename="number")]
        pub num: u32,
        /// Ratings loaded from flow file
        #[serde(skip_deserializing)]
        pub ratings: HashMap<String, Likert>
    }
}

/// Rating loaded from flow file
#[derive(Default, Serialize, Deserialize)]
pub struct Rating {
    /// Short version of prompt (used for radio button name)
    pub short: String,
    /// Answer given by experimenter
    pub answer: u8,
}

/// YYYYMMDD date
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Datestamp(pub u32);

/// Episode type (end-effector type)
#[derive(Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all="lowercase")]
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

impl<'f> FromForm<'f> for SurfaceData {
    type Error = rocket::Error;

    fn from_form(items: &mut FormItems<'f>, strict: bool) -> StdResult<Self, Self::Error> {
        let mut date = None;
        let mut flow = None;
        let mut num = None;
        let mut ratings = HashMap::new();

        macro_rules! arm {
            ($val:expr, $fld:ident) => {
                $fld = Some(FromFormValue::from_form_value($val)
                                .map_err(|e| {
                                    println!("\t=> Error parsing form val '{}': {:?}", stringify!($fld), e);
                                    rocket::Error::BadParse
                                })?)
            }
        }
        for (key, value) in items {
            match key.as_str() {
                "date" => arm!(value, date),
                "flow" => arm!(value, flow),
                "num" => arm!(value, num),
                s => {
                    if let Ok(n) = Likert::from_form_value(value) {
                        ratings.insert(s.to_string(), n);
                    } else if strict {
                        return Err(rocket::Error::BadParse);
                    }
                }
            }
        }

        if let (Some(date), Some(flow), Some(num)) = (date, flow, num) {
            Ok(SurfaceData {
                date: date,
                flow: flow,
                num: num,
                ratings
            })
        } else {
            println!("\t=> Error parsing form: missing values");
            Err(rocket::Error::BadParse)
        }
    }
}

impl FromStr for Likert {
    type Err = &'static str;

    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        match s.parse() {
            Ok(n @ 1...5) => Ok(Likert(n)),
            _ => Err("invalid 1-5 rating")
        }
    }
}

impl<'v> FromFormValue<'v> for Likert {
    type Error = <Self as FromStr>::Err;

    fn from_form_value(v: &'v RawStr) -> StdResult<Self, Self::Error> {
        v.as_str().parse()
    }
}

impl<'v> FromFormValue<'v> for Datestamp {
    type Error = ParseIntError;

    fn from_form_value(v: &'v RawStr) -> StdResult<Self, Self::Error> {
        Ok(v.as_str().parse().map(Datestamp)?)
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
            ratings: flow.states.iter()
                                .find(|s| s.name.starts_with("Wrap up")).unwrap() // TODO parse timestamps in flow crate
                                .script.iter()
                                       .filter_map(|&(ref cmd, _)| match *cmd {
                                           FlowCmd::Int { ref prompt, data: Some(d), .. } => Some((prompt.split('/').nth(1).unwrap().to_owned(), Likert(d as u8))),
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
    type Error = <FlowType as FromStr>::Err;

    fn from_param(param: &'a RawStr) -> StdResult<Self, Self::Error> {
        param.as_str().parse()
    }
}

impl<'v> FromFormValue<'v> for FlowType {
    type Error = <FlowType as FromStr>::Err;

    fn from_form_value(v: &'v RawStr) -> StdResult<Self, Self::Error> {
        v.as_str().parse()
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

