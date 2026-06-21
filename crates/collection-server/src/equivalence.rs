//! URI equivalence as a union-find (disjoint-set) structure.
//!
//! Equivalence is transitively closed by construction: `union(a, b)` merges sets,
//! and [`class`](EquivalenceSet::class) returns every URI in the same set. A
//! query on one URI can then return feedback anchored to any equivalent URI.
//!
//! We deliberately use union-find rather than Oxigraph SPARQL property paths
//! (the plan's suggestion): the closure is what we need, it is O(α(n)) per op,
//! has no external dependency, and is trivially testable. A SPARQL-backed
//! implementation behind the same API remains an option (see the M6 issue).

use std::collections::HashMap;

/// A set of URI equivalences plus their provenance.
#[derive(Default)]
pub struct EquivalenceSet {
    parent: HashMap<String, String>,
    /// Audit trail of asserted equivalences `(a, b, proof)`.
    proofs: Vec<(String, String, String)>,
}

impl EquivalenceSet {
    /// Create an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure(&mut self, x: &str) {
        if !self.parent.contains_key(x) {
            self.parent.insert(x.to_string(), x.to_string());
        }
    }

    fn find(&mut self, x: &str) -> String {
        self.ensure(x);
        let mut root = x.to_string();
        while self.parent[&root] != root {
            let grand = self.parent[&self.parent[&root]].clone();
            self.parent.insert(root.clone(), grand.clone()); // path halving
            root = self.parent[&root].clone();
        }
        root
    }

    /// Assert that `a` and `b` denote the same subject, with a `proof` string.
    pub fn union(&mut self, a: &str, b: &str, proof: impl Into<String>) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent.insert(ra, rb);
        }
        self.proofs
            .push((a.to_string(), b.to_string(), proof.into()));
    }

    /// All URIs equivalent to `uri` (including `uri` itself). For an unknown URI
    /// this is just `[uri]`.
    pub fn class(&mut self, uri: &str) -> Vec<String> {
        let root = self.find(uri);
        let keys: Vec<String> = self.parent.keys().cloned().collect();
        let mut out: Vec<String> = keys.into_iter().filter(|k| self.find(k) == root).collect();
        if !out.iter().any(|u| u == uri) {
            out.push(uri.to_string());
        }
        out.sort();
        out
    }

    /// The recorded proofs.
    pub fn proofs(&self) -> &[(String, String, String)] {
        &self.proofs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_uri_is_its_own_class() {
        let mut e = EquivalenceSet::new();
        assert_eq!(e.class("a"), vec!["a".to_string()]);
    }

    #[test]
    fn union_is_transitive() {
        let mut e = EquivalenceSet::new();
        e.union("a", "b", "manual");
        e.union("b", "c", "manual");
        let mut class = e.class("a");
        class.sort();
        assert_eq!(class, vec!["a", "b", "c"]);
        // From any member you reach the full class.
        assert_eq!(e.class("c").len(), 3);
    }

    #[test]
    fn disjoint_sets_stay_separate() {
        let mut e = EquivalenceSet::new();
        e.union("a", "b", "p");
        e.union("x", "y", "p");
        assert_eq!(e.class("a"), vec!["a", "b"]);
        assert_eq!(e.class("x"), vec!["x", "y"]);
        assert_eq!(e.proofs().len(), 2);
    }
}
