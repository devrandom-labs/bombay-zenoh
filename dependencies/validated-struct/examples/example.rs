#[cfg(feature = "serde_json")]
use validated_struct::ValidatedMap;

fn string_validator(new: &str) -> bool {
    !new.is_empty()
}
fn hi_validator(new: &Hi) -> bool {
    new.c().len() == *new.d()
}
validated_struct::validator! {
    /// Struct documentation works as expected, just make sure they're in the right spot
    #[recursive_attrs] // attributes bellow are added to each substructure, such as Hi
    /// Documentation is an attribute, so it WILL be passed around by #[recursive_attrs]
    #[repr(C)]
    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
    Hello {
        /// field documentation is given to both the getter an setter for said field
        a: String where (string_validator),
        /// `b` is valid iff `d == c.len()`
        b:
        Hi {
            c: Vec<f64>,
            d: usize
        } where (hi_validator),
        #[serde(default)]
        #[validated(recursive_accessors)]
        e: StringMap,
    }
}

#[derive(Clone, Default, Debug, serde::Deserialize, serde::Serialize)]
pub struct StringMap(std::collections::HashMap<String, String>);
impl<'a> validated_struct::ValidatedMapAssociatedTypes<'a> for StringMap {
    type Accessor = &'a dyn std::any::Any;
}
impl validated_struct::ValidatedMap for StringMap {
    fn insert<'d, D: serde::Deserializer<'d>>(
        &mut self,
        key: &str,
        value: D,
    ) -> Result<(), validated_struct::InsertionError>
    where
        validated_struct::InsertionError: From<D::Error>,
    {
        self.0
            .insert(key.into(), serde::Deserialize::deserialize(value)?);
        Ok(())
    }

    fn get<'a>(&'a self, key: &str) -> Result<&'a dyn std::any::Any, validated_struct::GetError> {
        self.0
            .get(key)
            .map(|f| f as &dyn std::any::Any)
            .ok_or(validated_struct::GetError::NoMatchingKey)
    }

    type Keys = Vec<String>;

    fn keys(&self) -> Self::Keys {
        self.0.keys().cloned().collect()
    }

    #[cfg(feature = "serde_json")]
    fn get_json(&self, key: &str) -> Result<String, validated_struct::GetError> {
        self.0.get(key).map_or_else(
            || Err(validated_struct::GetError::NoMatchingKey),
            |s| serde_json::to_string(s).map_err(|e| validated_struct::GetError::Other(e.into())),
        )
    }
}

#[cfg(feature = "serde_json")]
fn main() {
    let from_str = serde_json::Deserializer::from_str;
    let mut hello =
        Hello::from_deserializer(&mut from_str(r#"{"a": "hi", "b": {"c": [0.1], "d":1}}"#))
            .unwrap();
    hello.insert("a", &mut from_str("\"\"")).unwrap_err();
    hello.insert("a", &mut from_str("\"hello\"")).unwrap();
    hello
        .insert("b", &mut from_str(r#"{"c": [0.2, 0.1], "d":3}"#))
        .unwrap_err();
    hello
        .insert("b", &mut from_str(r#"{"c": [0.2, 0.1], "d":2}"#))
        .unwrap();
    hello.insert("b/c", &mut from_str("[0.1, 0.3]")).unwrap();
    hello.insert("e/c", &mut from_str("\"hello\"")).unwrap();
    println!("{:?}", hello);
    println!("json e/c: {}", hello.get_json("e/c").unwrap());
    println!("json b/c: {}", hello.get_json("b/c").unwrap());
}
#[cfg(not(feature = "serde_json"))]
fn main() {
    panic!("You must build this example with --features=serde_json for it to work")
}
