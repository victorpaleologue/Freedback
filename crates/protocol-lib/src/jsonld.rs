//! JSON-LD ingest (primary, not interop).
//!
//! The native wire format **is** W3C Web Annotation JSON-LD, so the server must
//! accept any conformant serialization — not only the exact byte-shape our serde
//! model emits. [`from_jsonld`] normalizes an incoming document into the
//! canonical [`Annotation`] model, tolerating the serialization freedom JSON-LD
//! allows over the pinned Freedback/anno contexts:
//!
//! - `@context` as a string or an array (ignored — terms are resolved by name);
//! - `body` / `target` as a single object or an array;
//! - `target` as a bare IRI string or a `SpecificResource` object;
//! - `type` as a string or array, with or without the known prefixes
//!   (`freedback:` / `schema:` / `oa:`) or as full IRIs (matched by local name);
//! - properties in compact-term form (`ratingValue`) or prefixed/expanded form
//!   (`schema:ratingValue`).
//!
//! Because everything is normalized to the model **before** the dedup id and
//! signature are computed, two different serializations of the same feedback
//! collapse to the same content address (see ADR 0007). Pure Rust, so it runs on
//! native and `wasm32`. Arbitrary third-party `@context`s (terms outside the
//! pinned vocabulary) still require a full JSON-LD processor — tracked as the
//! `json-ld`-crate extension.

use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::{Annotation, Body, Creator, Motivation, Selector, Signature, Target};

/// Parse a W3C Web Annotation JSON-LD document into the canonical model.
pub fn from_jsonld(doc: &Value) -> Result<Annotation> {
    let obj = doc
        .as_object()
        .ok_or(Error::MissingField("annotation object"))?;

    let types = obj
        .get("type")
        .or_else(|| obj.get("@type"))
        .map(type_locals)
        .unwrap_or_default();
    if !types.iter().any(|t| t == "Annotation") {
        return Err(Error::OutOfBounds(
            "type must include \"Annotation\"".to_string(),
        ));
    }

    let motivation = parse_motivation(get(obj, &["motivation", "oa:motivatedBy", "motivatedBy"]))?;
    let target = parse_target(
        get(obj, &["target", "oa:hasTarget", "hasTarget"]).ok_or(Error::MissingField("target"))?,
    )?;
    let body = parse_bodies(
        get(obj, &["body", "oa:hasBody", "hasBody"]).ok_or(Error::MissingField("body"))?,
    )?;

    let mut ann = Annotation::new(motivation, target, body);

    if let Some(c) = get(obj, &["creator", "dcterms:creator"]) {
        ann.creator = parse_creator(c);
    }
    if let Some(s) = get(obj, &["created", "dcterms:created"]).and_then(Value::as_str) {
        ann.created = Some(s.to_string());
    }
    if let Some(ct) = get(obj, &["conformsTo", "dcterms:conformsTo"]).and_then(Value::as_str) {
        ann.conforms_to = Some(ct.to_string());
    }
    if let Some(id) = obj
        .get("id")
        .or_else(|| obj.get("@id"))
        .and_then(Value::as_str)
    {
        ann.id = Some(id.to_string());
    }
    if let Some(sig) = get(obj, &["signature", "freedback:signature"]) {
        ann.signature = parse_signature(sig);
    }

    Ok(ann)
}

fn get<'a>(obj: &'a Map<String, Value>, aliases: &[&str]) -> Option<&'a Value> {
    aliases.iter().find_map(|k| obj.get(*k))
}

/// The local name of a term/IRI: the part after the last `:`/`/`/`#`.
fn local(s: &str) -> String {
    s.rsplit([':', '/', '#']).next().unwrap_or(s).to_string()
}

fn type_locals(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![local(s)],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(local).collect(),
        _ => vec![],
    }
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn parse_motivation(v: Option<&Value>) -> Result<Motivation> {
    let s = v
        .and_then(Value::as_str)
        .ok_or(Error::MissingField("motivation"))?;
    match local(s).as_str() {
        "assessing" => Ok(Motivation::Assessing),
        "commenting" => Ok(Motivation::Commenting),
        "tagging" => Ok(Motivation::Tagging),
        other => Err(Error::OutOfBounds(format!("unknown motivation: {other}"))),
    }
}

fn parse_target(v: &Value) -> Result<Target> {
    match v {
        Value::String(s) => Ok(Target::Iri(s.clone())),
        Value::Array(a) => parse_target(a.first().ok_or(Error::MissingField("target"))?),
        Value::Object(o) => {
            let source = get(o, &["source", "id", "@id", "oa:hasSource", "hasSource"])
                .and_then(Value::as_str)
                .ok_or(Error::MissingField("target.source"))?
                .to_string();
            match get(o, &["selector", "oa:hasSelector", "hasSelector"]) {
                Some(sel) => Ok(Target::Specific {
                    source,
                    selector: Box::new(parse_selector(sel)?),
                }),
                None => Ok(Target::Iri(source)),
            }
        }
        _ => Err(Error::OutOfBounds("target must be an IRI or object".into())),
    }
}

fn parse_selector(v: &Value) -> Result<Selector> {
    let o = v
        .as_object()
        .ok_or(Error::OutOfBounds("selector must be an object".into()))?;
    let types = o
        .get("type")
        .or_else(|| o.get("@type"))
        .map(type_locals)
        .unwrap_or_default();
    if types.iter().any(|t| t == "TextQuoteSelector") {
        Ok(Selector::TextQuoteSelector {
            exact: get(o, &["exact", "oa:exact"])
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            prefix: get(o, &["prefix", "oa:prefix"])
                .and_then(Value::as_str)
                .map(str::to_string),
            suffix: get(o, &["suffix", "oa:suffix"])
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    } else if types.iter().any(|t| t == "TextPositionSelector") {
        Ok(Selector::TextPositionSelector {
            start: get(o, &["start", "oa:start"]).and_then(num).unwrap_or(0.0) as u64,
            end: get(o, &["end", "oa:end"]).and_then(num).unwrap_or(0.0) as u64,
        })
    } else {
        Err(Error::OutOfBounds("unsupported selector type".into()))
    }
}

fn parse_bodies(v: &Value) -> Result<Vec<Body>> {
    match v {
        Value::Array(a) => a.iter().map(parse_body).collect(),
        single => Ok(vec![parse_body(single)?]),
    }
}

fn parse_body(v: &Value) -> Result<Body> {
    let o = v
        .as_object()
        .ok_or(Error::OutOfBounds("body must be an object".into()))?;
    let types = o
        .get("type")
        .or_else(|| o.get("@type"))
        .map(type_locals)
        .unwrap_or_default();
    let has = |t: &str| types.iter().any(|x| x == t);

    let rating = || get(o, &["ratingValue", "schema:ratingValue"]).and_then(num);
    let worst = |d: f64| {
        get(o, &["worstRating", "schema:worstRating"])
            .and_then(num)
            .unwrap_or(d)
    };
    let best = |d: f64| {
        get(o, &["bestRating", "schema:bestRating"])
            .and_then(num)
            .unwrap_or(d)
    };

    if has("StarRating") {
        Ok(Body::StarRating {
            value: rating().unwrap_or_default(),
            worst: worst(1.0),
            best: best(5.0),
        })
    } else if has("ScalarRating") {
        Ok(Body::ScalarRating {
            value: rating().unwrap_or_default(),
            worst: worst(0.0),
            best: best(1.0),
        })
    } else if has("ThumbRating") {
        Ok(Body::ThumbRating {
            up: rating().unwrap_or_default() >= 0.5,
        })
    } else if has("TextualBody") {
        let value = get(o, &["value", "rdf:value"])
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let purpose = get(o, &["purpose", "oa:hasPurpose", "hasPurpose"])
            .and_then(Value::as_str)
            .map(local)
            .unwrap_or_default();
        if purpose == "tagging" {
            Ok(Body::Tag { value })
        } else {
            Ok(Body::Comment { value })
        }
    } else {
        Err(Error::OutOfBounds("unrecognized body type".into()))
    }
}

fn parse_creator(v: &Value) -> Option<Creator> {
    match v {
        Value::String(s) => Some(Creator::new(s.clone())),
        Value::Object(o) => {
            let id = get(o, &["id", "@id"]).and_then(Value::as_str)?.to_string();
            let type_ = o
                .get("type")
                .or_else(|| o.get("@type"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(Creator { id, type_ })
        }
        _ => None,
    }
}

fn parse_signature(v: &Value) -> Option<Signature> {
    let o = v.as_object()?;
    Some(Signature {
        alg: get(o, &["alg", "freedback:alg"])
            .and_then(Value::as_str)?
            .to_string(),
        kid: get(o, &["kid", "freedback:kid"])
            .and_then(Value::as_str)?
            .to_string(),
        sig: get(o, &["sig", "freedback:sig"])
            .and_then(Value::as_str)?
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::dedup_id;

    fn canonical() -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![Body::star(4.0)],
        )
        .with_created("2026-06-21T10:00:00Z")
        .with_creator(Creator::new("did:key:k1"))
    }

    #[test]
    fn round_trips_our_own_serialization() {
        // from_jsonld(serialize(ann)) == ann — so normalizing our own clients'
        // POSTs is a no-op (signatures keep verifying).
        let ann = canonical();
        let value = serde_json::to_value(&ann).unwrap();
        assert_eq!(from_jsonld(&value).unwrap(), ann);
    }

    #[test]
    fn accepts_varied_serializations_with_same_dedup_id() {
        let canonical_id = dedup_id(&canonical()).unwrap();

        // A different-but-equivalent serialization: @context as a string, body as
        // a single object, target as an object, prefixed property names.
        let variant = serde_json::json!({
            "@context": "http://www.w3.org/ns/anno.jsonld",
            "type": "Annotation",
            "motivation": "oa:assessing",
            "creator": "did:key:k1",
            "created": "2026-06-21T10:00:00Z",
            "target": { "id": "https://example.com/item/1" },
            "body": {
                "type": ["freedback:StarRating", "schema:Rating"],
                "schema:ratingValue": 4,
                "schema:worstRating": 1,
                "schema:bestRating": 5
            },
            "conformsTo": "https://freedback.org/profile/1"
        });
        let parsed = from_jsonld(&variant).unwrap();
        assert_eq!(
            dedup_id(&parsed).unwrap(),
            canonical_id,
            "equivalent serializations must content-address identically"
        );
    }

    #[test]
    fn comment_variant_normalizes() {
        let doc = serde_json::json!({
            "@context": ["http://www.w3.org/ns/anno.jsonld"],
            "type": "Annotation",
            "motivation": "commenting",
            "target": "https://example.com/x",
            "body": { "type": "TextualBody", "value": "nice", "purpose": "commenting" }
        });
        let ann = from_jsonld(&doc).unwrap();
        assert!(matches!(ann.body.as_slice(), [Body::Comment { value }] if value == "nice"));
    }

    #[test]
    fn rejects_non_annotation() {
        let doc = serde_json::json!({ "type": "Thing", "body": [], "target": "x" });
        assert!(from_jsonld(&doc).is_err());
    }
}
