//! The users of a device and how its login is protected, read with the
//! getters of `LANConfigSecurity` and `X_AVM-DE_Auth` only.
//!
//! No passwords are read: the user list holds only names. The name of the
//! current user is masked like every login name.

use serde::Serialize;

use crate::generic::Error;
use crate::read::{Reader, Values};
use crate::status::{self, Right};

/// The service with the users and the login settings.
const SERVICE: &str = "LANConfigSecurity1";

/// A user of the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct User {
    /// The login name.
    pub name: String,
    /// Whether this user logged in last.
    pub last_login: bool,
}

/// How the login to the device is protected. Values the device does not
/// report are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Login {
    /// Whether the login without a password is allowed.
    pub anonymous: Option<bool>,
    /// Whether the login by a button press on the device is allowed.
    pub button: Option<bool>,
    /// Whether the password set at the factory is still active.
    pub default_password: Option<bool>,
    /// Whether changes need a confirmation on the device.
    pub two_factor: Option<bool>,
}

/// The users and the login settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Users {
    /// The users, or `None` if the device does not tell, usually because
    /// the current user may not read them.
    pub users: Option<Vec<User>>,
    /// What the current user may do, like `BoxAdmin: readwrite`.
    pub rights: Vec<Right>,
    pub login: Login,
    /// Weak settings, like `default password active`.
    pub hints: Vec<String>,
}

impl Users {
    /// Replaces the names of the users with `hidden`, for output that must
    /// not identify them.
    #[must_use]
    pub fn without_names(mut self, hidden: &str) -> Self {
        for user in self.users.iter_mut().flatten() {
            hidden.clone_into(&mut user.name);
        }
        self
    }
}

/// Reads the users and the login settings.
///
/// # Errors
///
/// The errors of reading other than missing parts, see
/// [`Reader::try_read`].
pub async fn users(reader: &mut Reader) -> Result<Users, Error> {
    let list = reader
        .try_read(SERVICE, "X_AVM-DE_GetUserList", &[])
        .await?
        .and_then(|values| values.text("NewX_AVM-DE_UserList"))
        .map(|xml| list(&xml));
    let rights = reader
        .try_read(SERVICE, "X_AVM-DE_GetCurrentUser", &[])
        .await?
        .and_then(|values| values.text("NewX_AVM-DE_CurrentUserRights"))
        .map_or_else(Vec::new, |xml| status::rights(&xml));
    let anonymous = reader
        .try_read(SERVICE, "X_AVM-DE_GetAnonymousLogin", &[])
        .await?
        .unwrap_or_default();
    let info = reader
        .try_read(SERVICE, "GetInfo", &[])
        .await?
        .unwrap_or_default();
    let auth = reader
        .try_read("X_AVM-DE_Auth1", "GetInfo", &[])
        .await?
        .unwrap_or_default();
    let login = login(&anonymous, &info, &auth);
    let hints = hints(&login);
    Ok(Users {
        users: list,
        rights,
        login,
        hints,
    })
}

/// The login settings from the answers of `X_AVM-DE_GetAnonymousLogin`,
/// `LANConfigSecurity:GetInfo` and `X_AVM-DE_Auth:GetInfo`.
fn login(anonymous: &Values, info: &Values, auth: &Values) -> Login {
    Login {
        anonymous: anonymous.flag("NewX_AVM-DE_AnonymousLoginEnabled"),
        button: anonymous.flag("NewX_AVM-DE_ButtonLoginEnabled"),
        default_password: info.flag("NewX_AVM-DE_IsDefaultPasswordActive"),
        two_factor: auth.flag("NewEnabled"),
    }
}

/// The weak login settings.
fn hints(login: &Login) -> Vec<String> {
    [
        (login.default_password, "default password active"),
        (login.anonymous, "login without password allowed"),
        (login.button, "login by button press allowed"),
        (
            login.two_factor.map(|on| !on),
            "two-factor confirmation off",
        ),
    ]
    .into_iter()
    .filter(|(weak, _)| *weak == Some(true))
    .map(|(_, hint)| hint.to_owned())
    .collect()
}

/// Parses a user list like
/// `<List><Username last_user="1">admin</Username></List>`. Unreadable
/// lists are empty.
fn list(xml: &str) -> Vec<User> {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    document
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("Username"))
        .filter_map(|node| {
            let name = node.text().unwrap_or_default().trim();
            (!name.is_empty()).then(|| User {
                name: name.to_owned(),
                last_login: node.attribute("last_user") == Some("1"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::values;

    #[test]
    fn parses_the_user_list() {
        let users = list(
            "<List><Username last_user=\"0\">alice</Username><Username\n\
             last_user=\"1\"> bob </Username><Username/><Other>x</Other></List>",
        );
        assert_eq!(
            users,
            [
                User {
                    name: "alice".to_owned(),
                    last_login: false,
                },
                User {
                    name: "bob".to_owned(),
                    last_login: true,
                },
            ]
        );
        assert_eq!(list("not xml"), []);
    }

    #[test]
    fn hints_at_weak_settings() {
        let login = login(
            &values(&[
                ("NewX_AVM-DE_AnonymousLoginEnabled", "1"),
                ("NewX_AVM-DE_ButtonLoginEnabled", "1"),
            ]),
            &values(&[("NewX_AVM-DE_IsDefaultPasswordActive", "1")]),
            &values(&[("NewEnabled", "0")]),
        );
        assert_eq!(
            hints(&login),
            [
                "default password active",
                "login without password allowed",
                "login by button press allowed",
                "two-factor confirmation off",
            ]
        );
        // Unknown settings give no hints.
        assert_eq!(hints(&Login::default()), Vec::<String>::new());
    }

    #[test]
    fn hides_the_names() {
        let users = Users {
            users: Some(list("<List><Username>alice</Username></List>")),
            ..Users::default()
        };
        let hidden = users.without_names("(hidden)");
        assert_eq!(hidden.users.unwrap()[0].name, "(hidden)");
        assert_eq!(Users::default().without_names("x").users, None);
    }
}
