//! `fritz users`: the users of the device and how its login is protected.

use std::io::Write;

use anyhow::Context as _;
use fritz_core::users::{self, Users};

use super::{on_off, pairs, reader, yes_no};
use crate::context::Context;
use crate::output::{Format, show};

/// What is shown for a value the device does not report.
const UNKNOWN: &str = "unknown";

/// Prints the users and the login settings.
pub(crate) async fn users(
    context: &Context,
    format: Format,
    out: &mut impl Write,
) -> anyhow::Result<()> {
    let mut reader = reader(context).await?;
    let read = users::users(&mut reader).await;
    let users = read.context("cannot read the users")?;
    show(&users, format, out, |users, out| {
        pairs(&users_pairs(users), out)
    })
}

/// The users and the login settings as lines.
fn users_pairs(users: &Users) -> Vec<(&'static str, Option<String>)> {
    let names = users.users.as_ref().map_or_else(
        || "not allowed".to_owned(),
        |list| {
            let names: Vec<String> = list
                .iter()
                .map(|user| {
                    if user.last_login {
                        format!("{} (last login)", user.name)
                    } else {
                        user.name.clone()
                    }
                })
                .collect();
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join(", ")
            }
        },
    );
    let rights: Vec<String> = users
        .rights
        .iter()
        .map(|r| format!("{}: {}", r.area, r.access))
        .collect();
    let rights = if rights.is_empty() {
        UNKNOWN.to_owned()
    } else {
        rights.join(", ")
    };
    let or_unknown = |value: Option<bool>, name: fn(bool) -> String| {
        Some(value.map_or_else(|| UNKNOWN.to_owned(), name))
    };
    let login = &users.login;
    let hints = Some(users.hints.join(", ")).filter(|h| !h.is_empty());
    vec![
        ("Users", Some(names)),
        ("Your rights", Some(rights)),
        (
            "Login without password",
            or_unknown(login.anonymous, yes_no),
        ),
        ("Login by button", or_unknown(login.button, yes_no)),
        (
            "Default password",
            or_unknown(login.default_password, yes_no),
        ),
        ("Two-factor", or_unknown(login.two_factor, on_off)),
        ("Hints", hints),
    ]
}

#[cfg(test)]
mod tests {
    use fritz_core::status::Right;
    use fritz_core::users::{Login, User};

    use super::*;

    fn text(users: &Users) -> String {
        let mut out = Vec::new();
        pairs(&users_pairs(users), &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn prints_users_and_hints() {
        let users = Users {
            users: Some(vec![
                User {
                    name: "admin".to_owned(),
                    last_login: true,
                },
                User {
                    name: "guest".to_owned(),
                    last_login: false,
                },
            ]),
            rights: vec![Right {
                area: "BoxAdmin".to_owned(),
                access: "readwrite".to_owned(),
            }],
            login: Login {
                anonymous: Some(true),
                button: Some(false),
                default_password: Some(true),
                two_factor: Some(true),
            },
            hints: vec![
                "default password active".to_owned(),
                "login without password allowed".to_owned(),
            ],
        };
        assert_eq!(
            text(&users),
            "Users                   admin (last login), guest\n\
             Your rights             BoxAdmin: readwrite\n\
             Login without password  yes\n\
             Login by button         no\n\
             Default password        yes\n\
             Two-factor              on\n\
             Hints                   default password active, login without password allowed\n"
        );
    }

    #[test]
    fn prints_what_is_not_reported() {
        assert_eq!(
            text(&Users::default()),
            "Users                   not allowed\n\
             Your rights             unknown\n\
             Login without password  unknown\n\
             Login by button         unknown\n\
             Default password        unknown\n\
             Two-factor              unknown\n"
        );
        let empty = Users {
            users: Some(Vec::new()),
            ..Users::default()
        };
        assert!(text(&empty).starts_with("Users                   none\n"));
    }
}
