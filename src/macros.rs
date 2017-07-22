macro_rules! handle {
    ($(#[$attr:meta])* $vis:vis fn $name:ident $params:tt -> $ret:ty { $($body:tt)* }) => {
        $(#[$attr])*
        $vis fn $name $params -> StdResult<$ret, Failure> {
            do catch { $($body)* }.map_err(|err: Error| {
                println!("ERROR: {:?}", err);
                let mut code = 500;
                match *err.kind() {
                    ErrorKind::Io(ref ioerr) | ErrorKind::IoOp(ref ioerr, ..) => match ioerr.kind() {
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
    (#[$method:ident($($route:tt)*)] $vis:vis fn $name:ident/$name_login:ident($user:ident: $user_ty:ty, $($params:tt)*) -> $ret:ty { $($body:tt)* }) => {
        #[$method($($route)*, rank=1)]
        #[allow(unused_variables)]
        $vis fn $name_login(uri: &URI, $($params)*) -> Redirect {
            Redirect::to(&format!("/login?uri={}", URI::percent_encode(uri.as_str())))
        }

        handle! {
            #[$method($($route)*)]
            $vis fn $name($user: $user_ty, $($params)*) -> $ret { $($body)* }
        }
    }
}

macro_rules! with_user {
    (
        $(#[$sattr:meta])*
        $svis:vis struct $sname:ident/$uname:ident<$utype:ty> {
            $(
                $(#[$fattr:meta])*
                $fvis:vis $fname:ident: $ftype:ty
            ),*
            $(,)*
        }
    ) => {
        $(#[$sattr])*
        $svis struct $sname {
            $(
                $(#[$fattr])*
                $fvis $fname: $ftype
            ),*
        }

        $(#[$sattr])*
        $svis struct $uname {
            user: $utype,
            $(
                $(#[$fattr])*
                $fvis $fname: $ftype
            ),*
        }

        impl $sname {
            #[allow(dead_code)]
            $svis fn with_user(self, user: $utype) -> $uname {
                $uname {
                    user,
                    $($fname: self.$fname),*
                }
            }
        }

        impl $uname {
            #[allow(dead_code)]
            $svis fn without_user(self) -> ($sname, $utype) {
                ($sname {
                     $($fname: self.$fname),*
                 },
                 self.user
                )
            }
        }
    }
}

