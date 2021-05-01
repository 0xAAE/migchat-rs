use std::{
    error::Error,
    fmt::{self, Display},
    str::FromStr,
};

tonic::include_proto!("migchat"); // The string specified here must match the proto package name

pub type ChatId = u64;
pub type UserId = u64;
pub type PostId = u64;

#[allow(dead_code)]
pub const NOT_USER_ID: UserId = 0;
#[allow(dead_code)]
pub const NOT_CHAT_ID: ChatId = 0;
#[allow(dead_code)]
pub const NOT_POST_ID: PostId = 0;

impl Display for User {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_name = !self.name.is_empty();
        let has_short = !self.short_name.is_empty();
        if has_name {
            if has_short {
                write!(f, "{} ({})", self.short_name, self.name)
            } else {
                write!(f, "{}", self.name)
            }
        } else if has_short {
            write!(f, "{}", self.short_name)
        } else {
            write!(f, "<not set>")
        }
    }
}

impl Display for UserInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_name = !self.name.is_empty();
        let has_short = !self.short_name.is_empty();
        if has_name {
            if has_short {
                write!(f, "{} ({})", self.short_name, self.name)
            } else {
                write!(f, "{}", self.name)
            }
        } else if has_short {
            write!(f, "{}", self.short_name)
        } else {
            write!(f, "<not set>")
        }
    }
}

#[derive(Debug)]
struct ParseUserError {
    text: String,
}

impl Error for ParseUserError {}

impl Display for ParseUserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)
    }
}

impl From<User> for UserInfo {
    fn from(user: User) -> Self {
        UserInfo {
            name: user.name,
            short_name: user.short_name,
        }
    }
}

impl FromStr for UserInfo {
    // the error must be owned as well
    type Err = Box<dyn std::error::Error>;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(&[',', ':', ';'][..]).collect();
        if parts.len() == 2 {
            Ok(UserInfo {
                short_name: parts[0].to_string(),
                name: parts[1].trim().to_string(),
            })
        } else {
            Err(Box::new(ParseUserError {
                text: s.to_string(),
            }))
        }
    }
}

#[test]
fn test_parse_user() {
    let input = "login, User Name";
    assert_eq!(
        input.parse::<UserInfo>().unwrap(),
        UserInfo {
            name: String::from("User Name"),
            short_name: String::from("login")
        }
    );
}

#[test]
fn test_display_user() {
    assert_eq!(
        format!(
            "{}",
            UserInfo {
                name: String::new(),
                short_name: String::new()
            }
        ),
        "<not set>"
    );
    assert_eq!(
        format!(
            "{}",
            UserInfo {
                name: String::from("Only Name"),
                short_name: String::new()
            }
        ),
        "Only Name"
    );
    assert_eq!(
        format!(
            "{}",
            UserInfo {
                name: String::new(),
                short_name: String::from("Login")
            }
        ),
        "Login"
    );
    assert_eq!(
        format!(
            "{}",
            UserInfo {
                name: String::from("User Name"),
                short_name: String::from("Login")
            }
        ),
        "Login (User Name)"
    );
}
