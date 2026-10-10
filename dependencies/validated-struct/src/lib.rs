pub use validated_struct_macros::*;

#[derive(Debug)]
pub enum InsertionError {
    SyncInsertNotAvailable,
    #[cfg(feature = "serde_json")]
    JsonErr(serde_json::Error),
    #[cfg(feature = "json5")]
    Json5Err(json5::Error),
    Str(&'static str),
    String(String),
}
impl std::error::Error for InsertionError {}
impl std::fmt::Display for InsertionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
impl InsertionError {
    pub fn sync_insert_not_available() -> Self {
        InsertionError::SyncInsertNotAvailable
    }
}
#[cfg(feature = "serde_json")]
impl From<serde_json::Error> for InsertionError {
    fn from(e: serde_json::Error) -> Self {
        InsertionError::JsonErr(e)
    }
}
#[cfg(feature = "json5")]
impl From<json5::Error> for InsertionError {
    fn from(e: json5::Error) -> Self {
        InsertionError::Json5Err(e)
    }
}
#[derive(Debug)]
pub enum GetError {
    NoMatchingKey,
    TypeMissMatch,
    #[cfg(feature = "serde_json")]
    Other(Box<dyn std::error::Error>),
}
impl std::error::Error for GetError {}
impl std::fmt::Display for GetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetError::NoMatchingKey | GetError::TypeMissMatch => write!(f, "{:?}", self),
            #[cfg(feature = "serde_json")]
            GetError::Other(e) => write!(f, "{}", e),
        }
    }
}
impl From<&'static str> for InsertionError {
    fn from(s: &'static str) -> Self {
        InsertionError::Str(s)
    }
}
impl From<String> for InsertionError {
    fn from(s: String) -> Self {
        InsertionError::String(s)
    }
}
pub trait ValidatedMapAssociatedTypes<'a> {
    type Accessor;
}
pub trait ValidatedMap: for<'a> ValidatedMapAssociatedTypes<'a> {
    fn insert<'d, D: serde::Deserializer<'d>>(
        &mut self,
        key: &str,
        value: D,
    ) -> Result<(), InsertionError>
    where
        InsertionError: From<D::Error>;
    fn get<'a>(
        &'a self,
        key: &str,
    ) -> Result<<Self as ValidatedMapAssociatedTypes<'a>>::Accessor, GetError>;
    #[cfg(feature = "serde_json")]
    fn get_json(&self, key: &str) -> Result<String, GetError>;
    #[cfg(feature = "json5")]
    fn insert_json5(&mut self, key: &str, value: &str) -> Result<(), InsertionError> {
        self.insert(key, &mut json5::Deserializer::from_str(value)?)
    }
    type Keys: IntoIterator<Item = String>;
    fn keys(&self) -> Self::Keys;
}
pub fn split_once(s: &str, pattern: char) -> (&str, &str) {
    let index = s.find(pattern).unwrap_or(s.len());
    let (l, r) = s.split_at(index);
    (l, if r.is_empty() { "" } else { &r[1..] })
}
