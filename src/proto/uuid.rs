use std::cmp::Eq;
use std::hash::{Hash, Hasher};

impl Hash for super::Uuid {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl Eq for super::Uuid {}
