use serde_json::Value;
use std::{collections::BTreeMap, env, fs, path::PathBuf};
fn pascal(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut c = part.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect()
}
fn rust_type(s: &Value, prefixes: &BTreeMap<String, String>) -> String {
    if let Some(a) = s["anyOf"].as_array() {
        if a.iter().any(|x| x["type"] == "null") {
            return format!(
                "Option<{}>",
                rust_type(a.iter().find(|x| x["type"] != "null").unwrap(), prefixes)
            );
        }
    }
    if let Some(a) = s["type"].as_array() {
        if a.iter().any(|x| x == "null") {
            let mut n = s.clone();
            n["type"] = a.iter().find(|x| *x != "null").unwrap().clone();
            return format!("Option<{}>", rust_type(&n, prefixes));
        }
    }
    if let Some(r) = s["$ref"].as_str() {
        return match r.rsplit('/').next().unwrap() {
            "UInt64" => "UInt64".into(),
            "Body" => "Body".into(),
            "BlobRef" => "BlobRef".into(),
            "SecretRef" => "SecretRef".into(),
            "Timestamp" => "Timestamp".into(),
            "Digest" => "Digest".into(),
            _ => "Value".into(),
        };
    }
    if let Some(p) = s["pattern"].as_str() {
        for (prefix, name) in prefixes {
            if p == format!("^{prefix}_[0-9a-f]{{32}}$") {
                return format!("Id<{name}>");
            }
        }
        if s.get("x-max").is_some() {
            return "UInt64".into();
        }
        if p == "^[0-9a-f]{64}$" {
            return "Digest".into();
        }
    }
    match s["type"].as_str() {
        Some("string") if s["format"] == "date-time" => "Timestamp".into(),
        Some("string") => "String".into(),
        Some("integer") => "i32".into(),
        Some("boolean") => "bool".into(),
        Some("array") => format!("Vec<{}>", rust_type(&s["items"], prefixes)),
        _ => "Value".into(),
    }
}
fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../contracts");
    let registry: Value =
        serde_json::from_slice(&fs::read(root.join("collection-registry.v1.json")).unwrap())
            .unwrap();
    let schemas: Value =
        serde_json::from_slice(&fs::read(root.join("entities.v1.schema.json")).unwrap()).unwrap();
    println!(
        "cargo:rerun-if-changed={}",
        root.join("collection-registry.v1.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("entities.v1.schema.json").display()
    );
    let collections = registry["collections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] != "read_projection")
        .collect::<Vec<_>>();
    let prefixes = collections
        .iter()
        .map(|c| {
            (
                c["id_prefix"].as_str().unwrap().to_owned(),
                pascal(c["collection"].as_str().unwrap()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut out = format!(
        "pub const GENERATED_ENTITY_COUNT:usize={};\n",
        collections.len()
    );
    // Optional nullable fields have three states: absent, explicit null and a
    // value. Preserve presence rather than adding nulls to old stored records.
    out += "fn deserialize_present<'de,D,T>(d:D)->std::result::Result<Option<T>,D::Error> where D:serde::Deserializer<'de>,T:Deserialize<'de>{T::deserialize(d).map(Some)}\n";
    for c in &collections {
        let name = pascal(c["collection"].as_str().unwrap());
        let collection = c["collection"].as_str().unwrap();
        let prefix = c["id_prefix"].as_str().unwrap();
        let schema = &schemas["$defs"][c["schema"].as_str().unwrap()];
        let data = if schema["properties"].get("data").is_some() {
            &schema["properties"]["data"]
        } else {
            schema
        };
        out += &format!(
            "#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct {name} {{\n"
        );
        for (field, s) in data["properties"].as_object().unwrap() {
            let rust = if [
                "type", "ref", "self", "match", "loop", "async", "where", "mod", "use", "move",
                "fn", "crate", "super", "in", "final", "try", "yield", "dyn", "abstract",
                "override", "virtual", "priv", "become", "box",
            ]
            .contains(&field.as_str())
            {
                format!("r#{field}")
            } else {
                field.clone()
            };
            let ty = rust_type(s, &prefixes);
            let required = data["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|name| name == field));
            if required {
                out += &format!("pub {rust}:{ty},\n");
            } else {
                out += "#[serde(default,skip_serializing_if=\"Option::is_none\",deserialize_with=\"deserialize_present\")]\n";
                out += &format!("pub {rust}:Option<{ty}>,\n");
            }
        }
        out += "}\n";
        out += &format!(
            "impl Entity for {name} {{ const COLLECTION:&'static str=\"{collection}\"; const PREFIX:&'static str=\"{prefix}\"; }}\n"
        );
    }
    out += "pub fn typed_roundtrip(record:&crate::Record)->crate::Result<()> {crate::validate_record(record)?;let data=match record.header.collection.as_str(){\n";
    for c in &collections {
        let n = pascal(c["collection"].as_str().unwrap());
        out += &format!(
            "\"{}\"=>serde_json::to_value(serde_json::from_value::<{n}>(record.data.clone())?)?,\n",
            c["collection"].as_str().unwrap()
        );
    }
    out += "_=>return Err(crate::Error::new(\"INVALID_PARAMS\",\"Unknown typed entity\")),};if data!=record.data{return Err(crate::Error::new(\"INVALID_PARAMS\",\"Typed roundtrip changed entity data\"));}Ok(())}\n";
    fs::write(
        PathBuf::from(env::var("OUT_DIR").unwrap()).join("entities.rs"),
        out,
    )
    .unwrap();
}
