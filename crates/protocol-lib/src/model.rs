//! The Freedback data model.
//!
//! INVARIANTS (from the technical plan / `CLAUDE.md`, never violate):
//!  1. The annotation is the envelope (W3C Web Annotation JSON-LD).
//!  2. Typed feedback lives in the BODY: `freedback:StarRating` /
//!     `freedback:ScalarRating` / `freedback:ThumbRating` are
//!     `rdfs:subClassOf schema:Rating`. Comments/tags reuse `oa:TextualBody`.
//!  3. Validation lives entirely in SHACL (see `validation.rs`), never here:
//!     this module models structure, not business rules. The light checks in
//!     [`Annotation::structural_check`] guard only what SHACL cannot (e.g. a
//!     body variant existing at all) and are a convenience, not the authority.

use serde::{Deserialize, Serialize};

use crate::context;
use crate::error::{Error, Result};

/// Motivation for an annotation. Rating motivations specialize `oa:assessing`
/// via `skos:broader` (declared in the ontology); comments/tags reuse the
/// standard `oa:commenting` / `oa:tagging` motivations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Motivation {
    /// `oa:assessing` — the umbrella motivation for all typed ratings.
    Assessing,
    /// `oa:commenting` — free-text commentary (`oa:TextualBody`).
    Commenting,
    /// `oa:tagging` — a tag (`oa:TextualBody`).
    Tagging,
    /// `oa:editing` — an issue / problem report (`oa:TextualBody`): the W3C
    /// motivation "for when the user intends to request a change or edit to
    /// the Target resource". This is the 2014 proto's `Issue` feedback type
    /// (ADR 0023) expressed with zero new vocabulary. NOTE: `oa:flagging` was
    /// considered but does NOT exist in the W3C Web Annotation vocabulary.
    Editing,
}

/// The agent that issued an annotation (the `creator`).
///
/// For the self-signed identity this is the portable issuer id derived from the
/// P-256 public key (federates). For app-managed OAuth identity it is an
/// app-scoped composite id `(app_id, user_id)` that does NOT federate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creator {
    /// Issuer IRI (e.g. `did:key:...` for self-signed, or an app-scoped URN).
    pub id: String,
    /// Optional creator type (`schema:Person`, `schema:SoftwareApplication`, ...).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

impl Creator {
    /// Build a creator from a bare issuer IRI.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            type_: None,
        }
    }
}

/// A selector that narrows a target to a part of a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Selector {
    /// `oa:TextQuoteSelector` — robust, content-anchored (Hypothesis model).
    TextQuoteSelector {
        exact: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        suffix: Option<String>,
    },
    /// `oa:TextPositionSelector` — character offsets (fragile but precise).
    TextPositionSelector { start: u64, end: u64 },
}

/// The target of an annotation: either a bare IRI or a specific resource with a
/// selector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Target {
    /// A whole resource, addressed by IRI.
    Iri(String),
    /// A `oa:SpecificResource`: a `source` IRI narrowed by a `selector`.
    Specific {
        source: String,
        selector: Box<Selector>,
    },
}

impl Target {
    /// The source IRI of the target, regardless of variant. This is the key
    /// used for per-URI indexing and equivalence.
    pub fn source(&self) -> &str {
        match self {
            Target::Iri(s) => s,
            Target::Specific { source, .. } => source,
        }
    }
}

/// A typed feedback body. Rating variants are `rdfs:subClassOf schema:Rating`
/// (INVARIANT 2). Custom (de)serialization keeps the JSON-LD `type` arrays and
/// schema.org property names exact.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// `freedback:StarRating` — discrete stars, default scale 1..=5.
    StarRating { value: f64, worst: f64, best: f64 },
    /// `freedback:ScalarRating` — continuous bounded scale (default 0.0..=1.0).
    ScalarRating { value: f64, worst: f64, best: f64 },
    /// `freedback:ThumbRating` — the only net-new vocabulary term. `up` => 1.0,
    /// `down` => 0.0 on a `[0,1]` schema:Rating scale.
    ThumbRating { up: bool },
    /// `oa:TextualBody` with `oa:commenting` purpose — free text.
    Comment { value: String },
    /// `oa:TextualBody` with `oa:tagging` purpose — a single tag.
    Tag { value: String },
    /// `oa:TextualBody` with `oa:editing` purpose — an issue / problem report
    /// (the third feedback kind from the 2014 proto, ADR 0023). A distinct
    /// variant (rather than reusing [`Body::Comment`]) so the wire `purpose`
    /// mirrors the annotation's motivation exactly like comments/tags do; the
    /// serialization stays an ordinary `oa:TextualBody` — zero new vocabulary.
    Issue { value: String },
}

impl Body {
    /// A star rating on the default 1..=5 scale.
    pub fn star(value: f64) -> Self {
        Body::StarRating {
            value,
            worst: 1.0,
            best: 5.0,
        }
    }
    /// A scalar rating on the default 0.0..=1.0 scale.
    pub fn scalar(value: f64) -> Self {
        Body::ScalarRating {
            value,
            worst: 0.0,
            best: 1.0,
        }
    }
    /// A thumbs-up / thumbs-down rating.
    pub fn thumb(up: bool) -> Self {
        Body::ThumbRating { up }
    }
    /// An issue / problem report (free text, `oa:editing`).
    pub fn issue(text: impl Into<String>) -> Self {
        Body::Issue { value: text.into() }
    }
}

// --- Body wire form -------------------------------------------------------
// We serialize to a stable JSON-LD shape: rating bodies carry a `type` array
// of `[freedback:<Kind>, schema:Rating]` plus schema.org rating properties;
// textual bodies are `oa:TextualBody` with a `purpose`.

#[derive(Serialize, Deserialize)]
struct BodyWire {
    #[serde(rename = "type")]
    type_: TypeField,
    #[serde(rename = "schema:ratingValue", skip_serializing_if = "Option::is_none")]
    rating_value: Option<f64>,
    #[serde(rename = "schema:worstRating", skip_serializing_if = "Option::is_none")]
    worst_rating: Option<f64>,
    #[serde(rename = "schema:bestRating", skip_serializing_if = "Option::is_none")]
    best_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    purpose: Option<String>,
}

/// `type` may be a single string or an array of strings in JSON-LD.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum TypeField {
    One(String),
    Many(Vec<String>),
}

impl TypeField {
    fn contains(&self, v: &str) -> bool {
        match self {
            TypeField::One(s) => s == v,
            TypeField::Many(xs) => xs.iter().any(|s| s == v),
        }
    }
}

impl Serialize for Body {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        let wire = match self {
            Body::StarRating { value, worst, best } => BodyWire {
                type_: TypeField::Many(vec!["freedback:StarRating".into(), "schema:Rating".into()]),
                rating_value: Some(*value),
                worst_rating: Some(*worst),
                best_rating: Some(*best),
                value: None,
                format: None,
                purpose: None,
            },
            Body::ScalarRating { value, worst, best } => BodyWire {
                type_: TypeField::Many(vec![
                    "freedback:ScalarRating".into(),
                    "schema:Rating".into(),
                ]),
                rating_value: Some(*value),
                worst_rating: Some(*worst),
                best_rating: Some(*best),
                value: None,
                format: None,
                purpose: None,
            },
            Body::ThumbRating { up } => BodyWire {
                type_: TypeField::Many(vec![
                    "freedback:ThumbRating".into(),
                    "schema:Rating".into(),
                ]),
                rating_value: Some(if *up { 1.0 } else { 0.0 }),
                worst_rating: Some(0.0),
                best_rating: Some(1.0),
                value: None,
                format: None,
                purpose: None,
            },
            Body::Comment { value } => BodyWire {
                type_: TypeField::One("TextualBody".into()),
                rating_value: None,
                worst_rating: None,
                best_rating: None,
                value: Some(value.clone()),
                format: Some("text/plain".into()),
                purpose: Some("commenting".into()),
            },
            Body::Tag { value } => BodyWire {
                type_: TypeField::One("TextualBody".into()),
                rating_value: None,
                worst_rating: None,
                best_rating: None,
                value: Some(value.clone()),
                format: Some("text/plain".into()),
                purpose: Some("tagging".into()),
            },
            Body::Issue { value } => BodyWire {
                type_: TypeField::One("TextualBody".into()),
                rating_value: None,
                worst_rating: None,
                best_rating: None,
                value: Some(value.clone()),
                format: Some("text/plain".into()),
                purpose: Some("editing".into()),
            },
        };
        wire.serialize(ser)
    }
}

impl<'de> Deserialize<'de> for Body {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        let w = BodyWire::deserialize(de)?;
        use serde::de::Error as _;
        if w.type_.contains("freedback:StarRating") {
            Ok(Body::StarRating {
                value: w.rating_value.unwrap_or_default(),
                worst: w.worst_rating.unwrap_or(1.0),
                best: w.best_rating.unwrap_or(5.0),
            })
        } else if w.type_.contains("freedback:ScalarRating") {
            Ok(Body::ScalarRating {
                value: w.rating_value.unwrap_or_default(),
                worst: w.worst_rating.unwrap_or(0.0),
                best: w.best_rating.unwrap_or(1.0),
            })
        } else if w.type_.contains("freedback:ThumbRating") {
            Ok(Body::ThumbRating {
                up: w.rating_value.unwrap_or_default() >= 0.5,
            })
        } else if w.type_.contains("TextualBody") {
            let value = w.value.unwrap_or_default();
            match w.purpose.as_deref() {
                Some("tagging") => Ok(Body::Tag { value }),
                Some("editing") => Ok(Body::Issue { value }),
                _ => Ok(Body::Comment { value }),
            }
        } else {
            Err(D::Error::custom("unrecognized body type"))
        }
    }
}

/// A detached signature over the canonical bytes of the annotation.
///
/// This is the self-signed identity proof (INVARIANT 4a). The signature is
/// computed over the JCS canonicalization of the annotation with `id` and
/// `signature` removed (see `canonical.rs`), so it is independent of the
/// server-assigned id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// JWS algorithm. Always `ES256` (ECDSA P-256 + SHA-256).
    pub alg: String,
    /// Key id: the issuer's public key (PEM/SEC1) — the portable issuer id.
    pub kid: String,
    /// base64url(no-pad) ECDSA signature over the canonical bytes.
    pub sig: String,
}

/// A Freedback annotation: the native wire envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// `@context`: pinned W3C anno context + the Freedback context.
    #[serde(rename = "@context")]
    pub context: serde_json::Value,
    /// Server-assigned id. Excluded from the dedup id and signed bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Always `"Annotation"`.
    #[serde(rename = "type")]
    pub type_: String,
    /// Motivation (assessing / commenting / tagging).
    pub motivation: Motivation,
    /// Issuer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator: Option<Creator>,
    /// `xsd:dateTime` (ISO 8601, UTC). Source of truth for the `iat` cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// The target resource (and optional selector).
    pub target: Target,
    /// One or more typed bodies. Always serialized as an array for determinism.
    pub body: Vec<Body>,
    /// `dcterms:conformsTo`: pins the validation profile.
    #[serde(rename = "conformsTo", skip_serializing_if = "Option::is_none")]
    pub conforms_to: Option<String>,
    /// `rights` (the W3C Web Annotation term for `dcterms:rights`): an IRI
    /// identifying the license the author distributes this feedback under
    /// (e.g. `https://creativecommons.org/licenses/by/4.0/`). Optional — when
    /// absent, the annotation is distributed under the serving server's default
    /// license (advertised in `/.well-known/freedback`, ADR 0022). When present
    /// it IS content: it participates in the canonical bytes, so the same
    /// feedback under a different license is a different statement (different
    /// dedup id, separately signed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rights: Option<String>,
    /// Self-signed identity proof. Excluded from the dedup id and signed bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
}

impl Annotation {
    /// Build a new annotation with the pinned context and profile.
    pub fn new(motivation: Motivation, target: Target, body: Vec<Body>) -> Self {
        Self {
            context: context::context_value(),
            id: None,
            type_: "Annotation".to_string(),
            motivation,
            creator: None,
            created: None,
            target,
            body,
            conforms_to: Some(context::PROFILE_URL.to_string()),
            rights: None,
            signature: None,
        }
    }

    /// Set the creator (issuer) and return self (builder style).
    pub fn with_creator(mut self, creator: Creator) -> Self {
        self.creator = Some(creator);
        self
    }

    /// Set the `created` timestamp (ISO 8601 UTC) and return self.
    pub fn with_created(mut self, iso8601: impl Into<String>) -> Self {
        self.created = Some(iso8601.into());
        self
    }

    /// Set the `rights` license IRI (data licensing, ADR 0022) and return self.
    pub fn with_rights(mut self, license_iri: impl Into<String>) -> Self {
        self.rights = Some(license_iri.into());
        self
    }

    /// The issued-at unix timestamp derived from `created`, used by the
    /// `/sync?gt_iat=...` cursor. Returns `None` if `created` is absent or
    /// unparseable.
    pub fn iat(&self) -> Option<i64> {
        let created = self.created.as_deref()?;
        let parsed =
            time::OffsetDateTime::parse(created, &time::format_description::well_known::Rfc3339)
                .ok()?;
        Some(parsed.unix_timestamp())
    }

    /// Minimal structural sanity check. NOT a substitute for SHACL validation;
    /// it only catches things that make the value un-processable at all.
    pub fn structural_check(&self) -> Result<()> {
        if self.type_ != "Annotation" {
            return Err(Error::OutOfBounds(format!(
                "type must be \"Annotation\", got {:?}",
                self.type_
            )));
        }
        if self.body.is_empty() {
            return Err(Error::MissingField("body"));
        }
        Ok(())
    }
}
