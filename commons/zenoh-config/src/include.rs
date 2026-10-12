//
// Copyright (c) 2023 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ZettaScale Zenoh Team, <zenoh@zettascale.tech>
//

use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Map, Value};
use zenoh_result::{bail, zerror, ZResult};

pub(crate) fn deserialize_from_file<T, P>(path: P) -> ZResult<T>
where
    for<'de> T: Deserialize<'de>,
    P: AsRef<Path>,
{
    match std::fs::File::open(path.as_ref()) {
        Ok(mut f) => {
            let mut content = String::new();
            if let Err(e) = f.read_to_string(&mut content) {
                bail!(e)
            }
            match path.as_ref()
                .extension()
                .map(|s| s.to_str().unwrap())
            {
                Some("json") | Some("json5") => match json5::Deserializer::from_str(&content) {
                    Ok(mut d) => T::deserialize(&mut d).map_err(|e| zerror!("JSON5 error: {}", e).into()),
                    Err(e) => Err(zerror!("JSON5 error: {}", e).into()),
                },
                Some("yaml") => {
                    let d = serde_yaml::Deserializer::from_str(&content);
                    T::deserialize(d).map_err(|e| zerror!("YAML error: {}", e).into())
                },
                Some(other) => bail!("Unsupported file type '.{}' (.json, .json5 and .yaml are supported)", other),
                None => bail!("Unsupported file type. File must have an extension (.json, .json5 and .yaml supported)")
            }
        }
        Err(e) => {
            bail!(e)
        }
    }
}

pub(crate) fn recursive_include<P>(
    title: &str, // path in format "filename::object.object.object -> filename::object.object... -> ..." for error reporting
    values: &mut Map<String, Value>,
    mut loop_detector: HashSet<PathBuf>,
    include_property_name: &str,
    local_path: P,
) -> ZResult<()>
where
    P: AsRef<Path>,
{
    // if include property is present, read the file and remove properties found in file from values
    let include_object = if let Some(include_path) = values.get(include_property_name) {
        let Some(include_path) = include_path.as_str() else {
            bail!(
                "{}.{} : property must have string type",
                title,
                include_property_name
            );
        };
        let include_path_adjusted = Path::new(local_path.as_ref()).join(include_path);
        let include_path_canonical = match include_path_adjusted.canonicalize() {
            Ok(p) => p,
            Err(e) => bail!(
                "{}.{} : failed to canonicalize path '{}' - {}",
                title,
                include_property_name,
                include_path,
                e
            ),
        };

        if !loop_detector.insert(include_path_canonical) {
            bail!(
                "{}.{} : loop detected while including file '{}'",
                title,
                include_property_name,
                include_path
            );
        }

        let Some(new_local_path) = include_path_adjusted.parent() else {
            bail!(
                "{}.{} : cannot get directory part for '{}' value",
                title,
                include_property_name,
                include_path
            );
        };
        let mut include_object: Value = match deserialize_from_file(&include_path_adjusted) {
            Ok(v) => v,
            Err(e) => bail!(
                "{}.{} : failed to read file '{}' - {}",
                title,
                include_property_name,
                include_path,
                e
            ),
        };
        let Some(include_values) = include_object.as_object_mut() else {
            bail!(
                "{}.{}: included file '{}' must contain an object",
                title,
                include_property_name,
                include_path
            );
        };
        let include_path = include_path.to_string();
        for (k, v) in include_values.iter_mut() {
            // disallow include property on the top level - this would overwrite the include property from the parent file
            if k == include_property_name {
                bail!(
                    "{}.{} -> {}::{} : include property on the top level is not allowed",
                    title,
                    include_property_name,
                    include_path,
                    k,
                );
            }

            values.remove(k);
            if let Some(include_values) = v.as_object_mut() {
                let title = format!("{title}.{include_property_name} -> {include_path}::{k}");
                recursive_include(
                    title.as_str(),
                    include_values,
                    loop_detector.clone(),
                    include_property_name,
                    new_local_path,
                )?;
            }
        }
        Some(include_object)
    } else {
        None
    };

    // process remaining object values
    for (k, v) in values.iter_mut() {
        if let Some(object) = v.as_object_mut() {
            let title = format!("{title}.{k}");
            recursive_include(
                title.as_str(),
                object,
                loop_detector.clone(),
                include_property_name,
                local_path.as_ref(),
            )?;
        }
    }

    // if external file was included, add it's content to values
    if let Some(mut include_values) = include_object {
        values.append(include_values.as_object_mut().unwrap());
    }

    Ok(())
}

#[cfg(test)]
mod yaml_include_tests {
    use std::{collections::HashSet, fs};

    use serde_json::{json, Map, Value};
    use tempfile::TempDir;

    use super::{deserialize_from_file, recursive_include};
    use crate::Config;

    #[test]
    fn yaml_includes_preserve_complete_overrides_and_relative_paths() {
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("leaf.json5"), "{port:7447}").unwrap();
        fs::write(
            directory.path().join("child.yaml"),
            "same: child\nnested: {__config__: leaf.json5, port: 7}\n",
        )
        .unwrap();
        let included = directory.path().join("child.yaml");
        let parent = directory.path().join("parent.yaml");
        let source =
            json!({"plugins":{"fixture":{"__config__":included,"same":"parent","local":5}}});
        fs::write(&parent, source.to_string()).unwrap();
        let actual = Config::from_file(&parent);
        let expected = json!({"plugins":{"fixture":{"__config__":included,"same":"child","local":5,"nested":{"__config__":"leaf.json5","port":7447}}}});
        let expected = Config::from_deserializer(&mut serde_json::Deserializer::from_str(
            &expected.to_string(),
        ))
        .unwrap();
        directory.close().unwrap();
        let actual = serde_json::to_value(actual.unwrap()).unwrap();
        let expected = serde_json::to_value(expected).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn yaml_include_values_preserve_scalars_duplicates_and_literal_merges() {
        for (yaml, expected) in [
            (
                "plain: NO\nboolean: true\nnull: null\nnumber: 0b11\n",
                r#"{"plain":"NO","boolean":true,"null":null,"number":3}"#,
            ),
            (
                "original: &v {port: 1}\nalias: *v\n",
                r#"{"original":{"port":1},"alias":{"port":1}}"#,
            ),
            (
                "base: &b {x: 1}\nmerged: {<<: *b, y: 2}\n",
                r#"{"base":{"x":1},"merged":{"<<":{"x":1},"y":2}}"#,
            ),
            ("duplicate: 1\nduplicate: 2\n", r#"{"duplicate":2}"#),
            (
                "maximum: 18446744073709551615\n",
                r#"{"maximum":18446744073709551615}"#,
            ),
        ] {
            let directory = TempDir::new().unwrap();
            let path = directory.path().join("values.yaml");
            fs::write(&path, yaml).unwrap();
            let actual = deserialize_from_file::<Value, _>(&path);
            let expected: Value = serde_json::from_str(expected).unwrap();
            directory.close().unwrap();
            let actual = actual.unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn yaml_includes_preserve_structural_io_cycle_and_extension_refusals() {
        for (name, contents, expected) in [
            ("nonobject.yaml", Some("[]"), "must contain an object"),
            ("syntax.yaml", Some("["), "failed to read file"),
            (
                "tail.yaml",
                Some("key: value\n---\n["),
                "failed to read file",
            ),
            (
                "top.yaml",
                Some("__config__: leaf.yaml"),
                "include property on the top level",
            ),
            (
                "cycle.yaml",
                Some("nested: {__config__: cycle.yaml}"),
                "loop detected",
            ),
            (
                "unsupported.yml",
                Some("{}"),
                "Unsupported file type '.yml'",
            ),
            ("missing.yaml", None, "failed to canonicalize path"),
        ] {
            let directory = TempDir::new().unwrap();
            if let Some(contents) = contents {
                fs::write(directory.path().join(name), contents).unwrap();
            }
            let mut actual: Map<String, Value> =
                serde_json::from_value(json!({"__config__":name})).unwrap();
            let outcome = recursive_include(
                "fixture",
                &mut actual,
                HashSet::new(),
                "__config__",
                directory.path(),
            );
            directory.close().unwrap();
            let refusal = outcome.unwrap_err();
            assert!(refusal.to_string().contains(expected), "{refusal}");
        }
        let mut values: Map<String, Value> = serde_json::from_str(r#"{"__config__":4}"#).unwrap();
        let refusal = recursive_include("fixture", &mut values, HashSet::new(), "__config__", ".")
            .unwrap_err();
        assert!(refusal
            .to_string()
            .contains("property must have string type"));
    }
}
