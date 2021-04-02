use crate::proto;

#[derive(Debug, PartialEq, Clone)]
pub(crate) struct User {
    pub id: proto::UserId,
    pub name: String,
    pub short_name: String,
}

impl User {
    pub fn make_proto(&self) -> proto::User {
        (*self).clone().into()
    }
}

impl From<User> for proto::User {
    fn from(u: User) -> proto::User {
        proto::User {
            session_id: u.id,
            name: u.name,
            short_name: u.short_name,
        }
    }
}
