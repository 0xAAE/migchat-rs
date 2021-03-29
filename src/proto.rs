use std::hash::{Hash, Hasher};

tonic::include_proto!("migchat"); // The string specified here must match the proto package name

impl Hash for UserInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.short_name.hash(state);
    }
}
