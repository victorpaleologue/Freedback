//! Deterministic model → RDF (N-Triples) mapping.
//!
//! This is the offline equivalent of JSON-LD expansion against the pinned
//! `@context`: because `protocol-lib` owns the context, we can emit the exact
//! triples a compliant JSON-LD processor would, without resolving any remote
//! context document. The output feeds the SHACL validator (see `validation.rs`)
//! and is pure Rust, so it works on both native and `wasm32`.
//!
//! The mapping is intentionally explicit. If the `@context` changes, this
//! mapping and `ontology/context.jsonld` must change together.

use crate::context::{DCTERMS, FREEDBACK, OA, SCHEMA};
use crate::model::{Annotation, Body, Motivation, Selector, Target};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_VALUE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#value";
const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";
const XSD_DATETIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Render an annotation as N-Triples. The annotation node is a blank node
/// `_:ann`; bodies/targets/selectors get stable blank-node ids.
pub fn to_ntriples(ann: &Annotation) -> String {
    let mut w = Writer::default();
    let s = "_:ann";

    w.type_(s, &iri(OA, "Annotation"));
    w.iri_obj(s, &iri(OA, "motivatedBy"), &motivation_iri(ann.motivation));

    if let Some(created) = &ann.created {
        w.typed_literal(s, &iri(DCTERMS, "created"), created, XSD_DATETIME);
    }

    // Data licensing (ADR 0022): `rights` maps to dcterms:rights. An IRI-shaped
    // value is emitted as an IRI node; anything else is emitted as a string
    // literal so the SHACL `sh:nodeKind sh:IRI` constraint (the authority,
    // INVARIANT 3) rejects it — this module models structure, not rules.
    if let Some(rights) = &ann.rights {
        if is_absolute_iri(rights) {
            w.iri_obj(s, &iri(DCTERMS, "rights"), rights);
        } else {
            w.string_literal(s, &iri(DCTERMS, "rights"), rights);
        }
    }

    // Target
    match &ann.target {
        Target::Iri(t) => w.iri_obj(s, &iri(OA, "hasTarget"), t),
        Target::Specific { source, selector } => {
            let tnode = "_:target";
            w.bnode_obj(s, &iri(OA, "hasTarget"), tnode);
            w.type_(tnode, &iri(OA, "SpecificResource"));
            w.iri_obj(tnode, &iri(OA, "hasSource"), source);
            write_selector(&mut w, tnode, selector);
        }
    }

    // Bodies
    for (i, body) in ann.body.iter().enumerate() {
        let b = format!("_:body{i}");
        w.bnode_obj(s, &iri(OA, "hasBody"), &b);
        write_body(&mut w, &b, body);
    }

    w.out
}

fn write_selector(w: &mut Writer, target: &str, selector: &Selector) {
    let snode = "_:selector";
    w.bnode_obj(target, &iri(OA, "hasSelector"), snode);
    match selector {
        Selector::TextQuoteSelector {
            exact,
            prefix,
            suffix,
        } => {
            w.type_(snode, &iri(OA, "TextQuoteSelector"));
            w.string_literal(snode, &iri(OA, "exact"), exact);
            if let Some(p) = prefix {
                w.string_literal(snode, &iri(OA, "prefix"), p);
            }
            if let Some(s) = suffix {
                w.string_literal(snode, &iri(OA, "suffix"), s);
            }
        }
        Selector::TextPositionSelector { start, end } => {
            w.type_(snode, &iri(OA, "TextPositionSelector"));
            w.typed_literal(
                snode,
                &iri(OA, "start"),
                &start.to_string(),
                "http://www.w3.org/2001/XMLSchema#nonNegativeInteger",
            );
            w.typed_literal(
                snode,
                &iri(OA, "end"),
                &end.to_string(),
                "http://www.w3.org/2001/XMLSchema#nonNegativeInteger",
            );
        }
    }
}

fn write_body(w: &mut Writer, b: &str, body: &Body) {
    match body {
        Body::StarRating { value, worst, best } => {
            rating(w, b, "StarRating", *value, *worst, *best)
        }
        Body::ScalarRating { value, worst, best } => {
            rating(w, b, "ScalarRating", *value, *worst, *best)
        }
        Body::ThumbRating { up } => {
            rating(w, b, "ThumbRating", if *up { 1.0 } else { 0.0 }, 0.0, 1.0)
        }
        Body::Comment { value } => {
            w.type_(b, &iri(OA, "TextualBody"));
            w.string_literal(b, RDF_VALUE, value);
            w.iri_obj(b, &iri(OA, "hasPurpose"), &iri(OA, "commenting"));
        }
        Body::Tag { value } => {
            w.type_(b, &iri(OA, "TextualBody"));
            w.string_literal(b, RDF_VALUE, value);
            w.iri_obj(b, &iri(OA, "hasPurpose"), &iri(OA, "tagging"));
        }
        Body::Issue { value } => {
            w.type_(b, &iri(OA, "TextualBody"));
            w.string_literal(b, RDF_VALUE, value);
            w.iri_obj(b, &iri(OA, "hasPurpose"), &iri(OA, "editing"));
        }
        Body::Reply { value } => {
            w.type_(b, &iri(OA, "TextualBody"));
            w.string_literal(b, RDF_VALUE, value);
            w.iri_obj(b, &iri(OA, "hasPurpose"), &iri(OA, "replying"));
        }
    }
}

fn rating(w: &mut Writer, b: &str, kind: &str, value: f64, worst: f64, best: f64) {
    w.type_(b, &iri(FREEDBACK, kind));
    w.type_(b, &iri(SCHEMA, "Rating"));
    w.typed_literal(
        b,
        &iri(SCHEMA, "ratingValue"),
        &fmt_double(value),
        XSD_DOUBLE,
    );
    w.typed_literal(
        b,
        &iri(SCHEMA, "worstRating"),
        &fmt_double(worst),
        XSD_DOUBLE,
    );
    w.typed_literal(b, &iri(SCHEMA, "bestRating"), &fmt_double(best), XSD_DOUBLE);
}

fn motivation_iri(m: Motivation) -> String {
    match m {
        Motivation::Assessing => iri(OA, "assessing"),
        Motivation::Commenting => iri(OA, "commenting"),
        Motivation::Tagging => iri(OA, "tagging"),
        Motivation::Editing => iri(OA, "editing"),
        Motivation::Replying => iri(OA, "replying"),
    }
}

fn iri(ns: &str, term: &str) -> String {
    format!("{ns}{term}")
}

/// Whether a string is shaped like an absolute IRI that can be written inside
/// an N-Triples `<…>` IRIREF: a scheme (RFC 3986 §3.1) followed by characters
/// the IRIREF production allows. Deliberately syntactic — SHACL's
/// `sh:nodeKind sh:IRI` is the validation authority; this only decides whether
/// the value is representable as an IRI node at all.
fn is_absolute_iri(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    let scheme_ok = !scheme.is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let chars_ok = rest.chars().chain(scheme.chars()).all(|c| {
        c as u32 > 0x20 && !matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\')
    });
    scheme_ok && chars_ok
}

/// Format a double as a canonical xsd:double lexical form with a decimal point.
fn fmt_double(v: f64) -> String {
    if v == v.trunc() && v.is_finite() {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

#[derive(Default)]
struct Writer {
    out: String,
}

impl Writer {
    fn type_(&mut self, subj: &str, class_iri: &str) {
        self.iri_obj(subj, RDF_TYPE, class_iri);
    }
    fn iri_obj(&mut self, subj: &str, pred: &str, obj_iri: &str) {
        self.push(subj, pred, &format!("<{obj_iri}>"));
    }
    fn bnode_obj(&mut self, subj: &str, pred: &str, bnode: &str) {
        self.push(subj, pred, bnode);
    }
    fn string_literal(&mut self, subj: &str, pred: &str, lit: &str) {
        self.typed_literal(subj, pred, lit, XSD_STRING);
    }
    fn typed_literal(&mut self, subj: &str, pred: &str, lit: &str, datatype: &str) {
        let obj = format!("\"{}\"^^<{}>", escape(lit), datatype);
        self.push(subj, pred, &obj);
    }
    fn push(&mut self, subj: &str, pred: &str, obj: &str) {
        let s = if subj.starts_with("_:") {
            subj.to_string()
        } else {
            format!("<{subj}>")
        };
        self.out.push_str(&format!("{s} <{pred}> {obj} .\n"));
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, Body, Motivation, Target};

    #[test]
    fn emits_rating_triples() {
        let ann = Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/x".into()),
            vec![Body::star(4.0)],
        )
        .with_created("2026-06-21T10:00:00Z");
        let nt = to_ntriples(&ann);
        assert!(nt.contains("<http://www.w3.org/ns/oa#Annotation>"));
        assert!(nt.contains("<https://freedback.net/ns#StarRating>"));
        assert!(nt.contains(
            "<http://schema.org/ratingValue> \"4.0\"^^<http://www.w3.org/2001/XMLSchema#double>"
        ));
        assert!(nt
            .contains("<http://www.w3.org/ns/oa#motivatedBy> <http://www.w3.org/ns/oa#assessing>"));
    }

    #[test]
    fn emits_rights_as_iri_and_non_iri_as_literal() {
        let base = || {
            Annotation::new(
                Motivation::Assessing,
                Target::Iri("https://example.com/x".into()),
                vec![Body::star(4.0)],
            )
        };
        // An IRI-shaped license becomes an IRI node (satisfies sh:nodeKind sh:IRI).
        let nt = to_ntriples(&base().with_rights("https://creativecommons.org/licenses/by/4.0/"));
        assert!(nt.contains(
            "<http://purl.org/dc/terms/rights> <https://creativecommons.org/licenses/by/4.0/>"
        ));
        // A non-IRI value is still representable — as a literal, which SHACL
        // (the validation authority) then rejects via sh:nodeKind sh:IRI.
        let nt = to_ntriples(&base().with_rights("not an iri"));
        assert!(nt.contains(
            "<http://purl.org/dc/terms/rights> \"not an iri\"^^<http://www.w3.org/2001/XMLSchema#string>"
        ));
        // Absent rights emits no dcterms:rights triple at all.
        let nt = to_ntriples(&base());
        assert!(!nt.contains("dc/terms/rights"));
    }

    #[test]
    fn emits_issue_triples() {
        // Issue / problem report (ADR 0023): plain oa:TextualBody triples with
        // the standard oa:editing motivation/purpose — zero new vocabulary.
        let ann = Annotation::new(
            Motivation::Editing,
            Target::Iri("https://example.com/x".into()),
            vec![Body::issue("broken link")],
        );
        let nt = to_ntriples(&ann);
        assert!(nt.contains("<http://www.w3.org/ns/oa#TextualBody>"));
        assert!(nt.contains("\"broken link\"^^<http://www.w3.org/2001/XMLSchema#string>"));
        assert!(
            nt.contains("<http://www.w3.org/ns/oa#motivatedBy> <http://www.w3.org/ns/oa#editing>")
        );
        assert!(
            nt.contains("<http://www.w3.org/ns/oa#hasPurpose> <http://www.w3.org/ns/oa#editing>")
        );
    }

    #[test]
    fn emits_comment_triples() {
        let ann = Annotation::new(
            Motivation::Commenting,
            Target::Iri("https://example.com/x".into()),
            vec![Body::Comment {
                value: "nice".into(),
            }],
        );
        let nt = to_ntriples(&ann);
        assert!(nt.contains("<http://www.w3.org/ns/oa#TextualBody>"));
        assert!(nt.contains("\"nice\"^^<http://www.w3.org/2001/XMLSchema#string>"));
        assert!(nt.contains("<http://www.w3.org/ns/oa#commenting>"));
    }
}
