//! Shapes-driven SHACL-Core-subset validation (dual-target).
//!
//! INVARIANT 3: all validation rules live in `ontology/shapes.ttl`. This module
//! is a generic *interpreter* of those shapes — it hardcodes no bounds. It
//! parses the SHACL shapes graph and the annotation's RDF graph (both with the
//! pure-Rust `oxttl` parser) and evaluates the constraint components our profile
//! uses.
//!
//! ## Why a subset interpreter and not rudof's `shacl_validation`
//! The rudof 0.2.x crate set does not currently resolve to a consistent,
//! compilable dependency graph: the `iri_s` → `rudof_iri` rename straddles
//! patch releases, so caret resolution mixes `shacl_ast` (old `iri_s`) with
//! `rudof_rdf` (new `rudof_iri`) and fails to build. Rather than depend on an
//! unstable graph, we interpret the shapes ourselves for the components we use.
//! This is **shapes-driven** (swap `shapes.ttl` → behavior changes), compiles to
//! native *and* `wasm32` (widgets can pre-validate), and is isolated behind the
//! [`Validator`] API so a full external engine can replace it later. See
//! `docs/adr/0004-validation-in-shacl.md`.
//!
//! ## Supported constraint components
//! `sh:targetClass`, `sh:path`, `sh:minCount`, `sh:maxCount`, `sh:datatype`,
//! `sh:minInclusive`, `sh:maxInclusive`, `sh:minLength`, `sh:in`,
//! `sh:lessThanOrEquals` (sibling-property comparison — custom rating scales,
//! ADR 0009), `sh:message`. Anything else in `shapes.ttl` is ignored
//! (documented limitation).

use std::collections::BTreeMap;

use oxrdf::{NamedOrBlankNode, Term, Triple};
use oxttl::{NTriplesParser, TurtleParser};

use crate::error::{Error, Result};
use crate::model::Annotation;
use crate::rdf;

/// The SHACL shapes for profile `https://freedback.net/profile/1`, embedded at
/// compile time so the validator is self-contained.
pub const SHAPES_TTL: &str = include_str!("../../../ontology/shapes.ttl");

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

/// Outcome of validating an annotation against the SHACL shapes.
#[derive(Debug, Clone)]
pub struct ValidationOutcome {
    /// True iff no constraint was violated.
    pub conforms: bool,
    /// Human-readable violation messages (empty when `conforms`).
    pub violations: Vec<String>,
}

impl ValidationOutcome {
    /// Turn a non-conforming outcome into an [`Error::Validation`].
    pub fn into_result(self) -> Result<()> {
        if self.conforms {
            Ok(())
        } else {
            Err(Error::Validation(self.violations.join("; ")))
        }
    }
}

/// An owned RDF object value (avoids oxrdf borrow lifetimes during traversal).
#[derive(Debug, Clone, PartialEq)]
enum Obj {
    /// IRI or blank node, keyed by string (`<iri>` or `_:id`).
    Node(String),
    /// Literal with lexical value + datatype IRI.
    Lit { value: String, datatype: String },
}

impl Obj {
    fn as_node(&self) -> Option<&str> {
        match self {
            Obj::Node(s) => Some(s),
            _ => None,
        }
    }
    fn as_num(&self) -> Option<f64> {
        match self {
            Obj::Lit { value, .. } => value.parse::<f64>().ok(),
            Obj::Node(_) => None,
        }
    }
}

/// A minimal triple graph indexed for `(subject, predicate) -> [object]`.
#[derive(Default)]
struct Graph {
    spo: BTreeMap<(String, String), Vec<Obj>>,
}

impl Graph {
    fn parse_turtle(input: &str) -> Result<Self> {
        let mut g = Graph::default();
        for t in TurtleParser::new().for_slice(input.as_bytes()) {
            let t = t.map_err(|e| Error::Validation(format!("turtle parse: {e}")))?;
            g.insert(t);
        }
        Ok(g)
    }

    fn parse_ntriples(input: &str) -> Result<Self> {
        let mut g = Graph::default();
        for t in NTriplesParser::new().for_slice(input.as_bytes()) {
            let t = t.map_err(|e| Error::Validation(format!("ntriples parse: {e}")))?;
            g.insert(t);
        }
        Ok(g)
    }

    fn insert(&mut self, t: Triple) {
        let subj = node_key(&t.subject);
        let pred = t.predicate.as_str().to_string();
        let obj = term_to_obj(&t.object);
        self.spo.entry((subj, pred)).or_default().push(obj);
    }

    fn objects(&self, subj: &str, pred: &str) -> &[Obj] {
        self.spo
            .get(&(subj.to_string(), pred.to_string()))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// All subjects (keys) declared with `rdf:type class_iri`.
    fn subjects_of_type(&self, class_iri: &str) -> Vec<String> {
        let mut out = Vec::new();
        for ((s, p), objs) in &self.spo {
            if p == RDF_TYPE && objs.iter().any(|o| o.as_node() == Some(class_iri)) {
                out.push(s.clone());
            }
        }
        out
    }

    /// Resolve an RDF list (`rdf:first`/`rdf:rest`) headed at `head` into values.
    fn rdf_list(&self, head: &str) -> Vec<Obj> {
        let mut out = Vec::new();
        let mut cur = head.to_string();
        while cur != RDF_NIL {
            let Some(first) = self.objects(&cur, RDF_FIRST).first() else {
                break;
            };
            out.push(first.clone());
            match self.objects(&cur, RDF_REST).first().and_then(Obj::as_node) {
                Some(next) => cur = next.to_string(),
                None => break,
            }
        }
        out
    }
}

fn sh(term: &str) -> String {
    format!("{SH}{term}")
}

fn node_key(n: &NamedOrBlankNode) -> String {
    match n {
        NamedOrBlankNode::NamedNode(nn) => nn.as_str().to_string(),
        NamedOrBlankNode::BlankNode(bn) => format!("_:{}", bn.as_str()),
    }
}

fn term_to_obj(t: &Term) -> Obj {
    match t {
        Term::NamedNode(nn) => Obj::Node(nn.as_str().to_string()),
        Term::BlankNode(bn) => Obj::Node(format!("_:{}", bn.as_str())),
        Term::Literal(l) => Obj::Lit {
            value: l.value().to_string(),
            datatype: l.datatype().as_str().to_string(),
        },
        #[allow(unreachable_patterns)]
        _ => Obj::Lit {
            value: String::new(),
            datatype: String::new(),
        },
    }
}

/// A reusable validator holding the parsed shapes graph (`Send + Sync`).
#[derive(Clone)]
pub struct Validator {
    shapes_ttl: String,
}

impl Default for Validator {
    fn default() -> Self {
        Self {
            shapes_ttl: SHAPES_TTL.to_string(),
        }
    }
}

impl Validator {
    /// Build a validator from a custom SHACL shapes graph (Turtle).
    pub fn from_shapes_ttl(shapes_ttl: impl Into<String>) -> Self {
        Self {
            shapes_ttl: shapes_ttl.into(),
        }
    }

    /// Validate an annotation against the shapes.
    pub fn validate(&self, ann: &Annotation) -> Result<ValidationOutcome> {
        self.validate_ntriples(&rdf::to_ntriples(ann))
    }

    /// Validate a raw N-Triples data graph against the shapes.
    pub fn validate_ntriples(&self, data: &str) -> Result<ValidationOutcome> {
        let shapes = Graph::parse_turtle(&self.shapes_ttl)?;
        let graph = Graph::parse_ntriples(data)?;
        let mut violations = Vec::new();

        for shape in shapes.subjects_of_type(&sh("NodeShape")) {
            // Collect target classes for this node shape.
            let target_classes: Vec<&str> = shapes
                .objects(&shape, &sh("targetClass"))
                .iter()
                .filter_map(Obj::as_node)
                .collect();
            if target_classes.is_empty() {
                continue;
            }

            // Focus nodes = data subjects typed with any target class.
            let mut focus: Vec<String> = Vec::new();
            for tc in &target_classes {
                focus.extend(graph.subjects_of_type(tc));
            }

            // Each property shape (blank node) under sh:property.
            for ps in shapes.objects(&shape, &sh("property")) {
                let Some(ps) = ps.as_node() else { continue };
                let Some(path) = shapes
                    .objects(ps, &sh("path"))
                    .first()
                    .and_then(Obj::as_node)
                else {
                    continue;
                };
                for f in &focus {
                    eval_property(&shapes, &graph, ps, path, f, &mut violations);
                }
            }
        }

        Ok(ValidationOutcome {
            conforms: violations.is_empty(),
            violations,
        })
    }
}

/// Evaluate one property shape against one focus node.
fn eval_property(
    shapes: &Graph,
    data: &Graph,
    ps: &str,
    path: &str,
    focus: &str,
    out: &mut Vec<String>,
) {
    let values = data.objects(focus, path);
    let msg = || {
        shapes
            .objects(ps, &sh("message"))
            .first()
            .and_then(|o| match o {
                Obj::Lit { value, .. } => Some(value.clone()),
                _ => None,
            })
            .unwrap_or_else(|| format!("constraint on <{path}> violated"))
    };

    // sh:minCount / sh:maxCount
    if let Some(min) = shapes
        .objects(ps, &sh("minCount"))
        .first()
        .and_then(Obj::as_num)
    {
        if (values.len() as f64) < min {
            out.push(msg());
        }
    }
    if let Some(max) = shapes
        .objects(ps, &sh("maxCount"))
        .first()
        .and_then(Obj::as_num)
    {
        if (values.len() as f64) > max {
            out.push(msg());
        }
    }

    // Value-level constraints.
    let datatype = shapes
        .objects(ps, &sh("datatype"))
        .first()
        .and_then(Obj::as_node)
        .map(str::to_string);
    let min_incl = shapes
        .objects(ps, &sh("minInclusive"))
        .first()
        .and_then(Obj::as_num);
    let max_incl = shapes
        .objects(ps, &sh("maxInclusive"))
        .first()
        .and_then(Obj::as_num);
    let min_len = shapes
        .objects(ps, &sh("minLength"))
        .first()
        .and_then(Obj::as_num);
    let in_set: Option<Vec<Obj>> = shapes
        .objects(ps, &sh("in"))
        .first()
        .and_then(Obj::as_node)
        .map(|head| shapes.rdf_list(head));

    for v in values {
        if let Some(dt) = &datatype {
            let ok = matches!(v, Obj::Lit { datatype, .. } if datatype == dt);
            if !ok {
                out.push(msg());
                continue;
            }
        }
        if let Some(min) = min_incl {
            if v.as_num().map(|n| n < min).unwrap_or(true) {
                out.push(msg());
            }
        }
        if let Some(max) = max_incl {
            if v.as_num().map(|n| n > max).unwrap_or(true) {
                out.push(msg());
            }
        }
        if let Some(min) = min_len {
            let len = match v {
                Obj::Lit { value, .. } => value.chars().count() as f64,
                Obj::Node(s) => s.chars().count() as f64,
            };
            if len < min {
                out.push(msg());
            }
        }
        if let Some(set) = &in_set {
            // Match by numeric value when possible, else by exact value.
            let hit = set
                .iter()
                .any(|allowed| match (allowed.as_num(), v.as_num()) {
                    (Some(a), Some(b)) => (a - b).abs() < f64::EPSILON,
                    _ => allowed == v,
                });
            if !hit {
                out.push(msg());
            }
        }
    }

    // sh:lessThanOrEquals (SHACL Core): every value of `path` must be <= every
    // value of the referenced sibling property on the same focus node. This is
    // what lets a rating be validated against its OWN declared worst/best scale
    // (custom scales) — see ADR 0009.
    if let Some(other) = shapes
        .objects(ps, &sh("lessThanOrEquals"))
        .first()
        .and_then(Obj::as_node)
    {
        let others: Vec<f64> = data
            .objects(focus, other)
            .iter()
            .filter_map(Obj::as_num)
            .collect();
        for v in values.iter().filter_map(Obj::as_num) {
            if others.iter().any(|o| v > *o) {
                out.push(msg());
            }
        }
    }
}

/// Validate an annotation against the default Freedback profile shapes.
pub fn validate_annotation(ann: &Annotation) -> Result<ValidationOutcome> {
    Validator::default().validate(ann)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Annotation, Body, Motivation, Target};

    fn ann_with(body: Body) -> Annotation {
        Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/item/1".into()),
            vec![body],
        )
        .with_created("2026-06-21T10:00:00Z")
    }

    #[test]
    fn valid_star_conforms() {
        let out = validate_annotation(&ann_with(Body::star(4.0))).unwrap();
        assert!(out.conforms, "expected conforms, got {:?}", out.violations);
    }

    #[test]
    fn out_of_range_star_is_rejected() {
        let out = validate_annotation(&ann_with(Body::StarRating {
            value: 7.0,
            worst: 1.0,
            best: 5.0,
        }))
        .unwrap();
        assert!(!out.conforms);
        assert!(out.violations.iter().any(|m| m.contains("[1,5]")));
    }

    #[test]
    fn scalar_out_of_range_rejected() {
        // 5 is outside the declared [0,1] scale.
        let out = validate_annotation(&ann_with(Body::ScalarRating {
            value: 5.0,
            worst: 0.0,
            best: 1.0,
        }))
        .unwrap();
        assert!(!out.conforms);
    }

    #[test]
    fn custom_scalar_scale_conforms() {
        // A custom scale (0..10) with an in-range value is valid.
        let out = validate_annotation(&ann_with(Body::ScalarRating {
            value: 7.0,
            worst: 0.0,
            best: 10.0,
        }))
        .unwrap();
        assert!(
            out.conforms,
            "custom scale should conform: {:?}",
            out.violations
        );
    }

    #[test]
    fn custom_scalar_above_best_rejected() {
        // Above the body's own bestRating → rejected.
        let out = validate_annotation(&ann_with(Body::ScalarRating {
            value: 11.0,
            worst: 0.0,
            best: 10.0,
        }))
        .unwrap();
        assert!(!out.conforms);
        assert!(out.violations.iter().any(|m| m.contains("bestRating")));
    }

    #[test]
    fn custom_scalar_below_worst_rejected() {
        // Below the body's own worstRating → rejected.
        let out = validate_annotation(&ann_with(Body::ScalarRating {
            value: -1.0,
            worst: 0.0,
            best: 10.0,
        }))
        .unwrap();
        assert!(!out.conforms);
        assert!(out.violations.iter().any(|m| m.contains("worstRating")));
    }

    #[test]
    fn thumb_values_constrained() {
        let up = validate_annotation(&ann_with(Body::thumb(true))).unwrap();
        assert!(up.conforms, "thumb up should conform: {:?}", up.violations);
        let down = validate_annotation(&ann_with(Body::thumb(false))).unwrap();
        assert!(
            down.conforms,
            "thumb down should conform: {:?}",
            down.violations
        );
    }

    #[test]
    fn empty_comment_is_rejected() {
        let out = validate_annotation(&ann_with(Body::Comment {
            value: String::new(),
        }))
        .unwrap();
        assert!(!out.conforms);
    }

    #[test]
    fn valid_comment_conforms() {
        let out = validate_annotation(&Annotation::new(
            Motivation::Commenting,
            Target::Iri("https://example.com/x".into()),
            vec![Body::Comment {
                value: "great".into(),
            }],
        ))
        .unwrap();
        assert!(out.conforms, "expected conforms, got {:?}", out.violations);
    }
}
