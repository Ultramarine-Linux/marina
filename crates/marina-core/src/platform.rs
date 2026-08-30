/// A platform represented in the library.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Platform {
    pub slug: String,
    pub name: String,
}

impl Platform {
    pub fn new(slug: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            name: name.into(),
        }
    }
}
