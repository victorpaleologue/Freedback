//! Range-based set reconciliation (negentropy / NIP-77), pure Rust + `wasm32`.
//!
//! Backdated annotations break the `/sync?gt_iat=` cursor: an item with an
//! `iat` below the cursor is invisible to an incremental pull, so the
//! advanced-client used a from-scratch **full pull** (`reconcile_full`) to catch
//! them — O(all). This module implements the efficient O(diff) alternative, the
//! shape of Nostr's [NIP-77] negentropy protocol, over the per-`(server,target)`
//! set of content-addressed dedup ids.
//!
//! ## The protocol in one paragraph
//!
//! Both sides hold a set of [`Item`]s — a `(timestamp, id)` pair, the `id` being
//! the annotation's dedup id. Each side **sorts** its set by `(timestamp, id)`
//! (NIP-77 orders by `(created_at, id)`). The initiator proposes a covering set
//! of ranges; for each range it sends either a **fingerprint** (a cheap digest
//! of the ids in that range) or, when the range is already small, the
//! **explicit id list**. The responder compares each range against its own set:
//! a matching fingerprint settles the range (no transfer); a mismatch is
//! **split** into sub-ranges that recurse, and once a range is small enough the
//! responder answers with its explicit ids so the initiator can diff them
//! directly into `have` (only-initiator) and `need` (only-responder). The
//! initiator then fetches only the `need` ids. Bytes on the wire scale with the
//! number of *differing* items, not the set size.
//!
//! ## Framing (our choice — we do not match NIP-77's wire bytes)
//!
//! NIP-77 is a binary, varint-packed, stateful streaming protocol designed for
//! Nostr's persistent relay connection. Freedback is **HTTP/1.1 batch, not
//! real-time** (INVARIANT 7), so we keep the negentropy *algorithm* (sorted
//! sets, range fingerprints, recursive split, IdList mode) but frame each round
//! as a stateless JSON request/response over `POST /negentropy`:
//!
//! - **Fingerprint** = lowercase-hex `SHA-256` over the concatenated raw bytes
//!   of every dedup id in the range, prefixed by the count (so a range and a
//!   strict superset never collide). This is a "secure hash of ids in a range"
//!   per the issue; we use SHA-256 rather than NIP-77's addition-mod-`2^256`
//!   fingerprint because Freedback already depends on `sha2` everywhere and it
//!   keeps the wasm bundle lean — collision resistance is what matters here, not
//!   incremental updatability.
//! - A [`Message`] is a list of [`RangeMsg`]s, each either `Fingerprint{range,fp}`
//!   or `IdList{range, ids}`. The responder replies with another [`Message`].
//! - The client drives rounds to a fixpoint (no more `Fingerprint` mismatches),
//!   accumulating `need` ids, then bulk-fetches them. Each side is **read-only**
//!   over its set within a round, so the exchange is naturally stateless.
//!
//! [NIP-77]: https://github.com/nostr-protocol/nips/blob/master/77.md

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How many ids a range may hold before we send the explicit list instead of a
/// fingerprint (NIP-77's IdList threshold). A mismatching range above this is
/// split into [`BUCKETS`] sub-ranges; at or below it we send ids so the peer can
/// diff directly. Small enough to bound per-message size, large enough that a
/// handful of differences resolve in one or two rounds.
pub const ID_LIST_THRESHOLD: usize = 16;

/// Fan-out when splitting a mismatching range into sub-ranges. Each round shrinks
/// a differing region by this factor, so the depth is `log_BUCKETS(set_size)`.
pub const BUCKETS: usize = 16;

/// One element of a reconciled set: a timestamp and a content-addressed id.
///
/// Ordering is `(timestamp, id)` — the NIP-77 sort key — so both peers derive an
/// identical canonical order from the same set and address the same ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// The annotation's issued-at unix timestamp (its `iat`).
    pub timestamp: i64,
    /// The annotation's content-addressed dedup id (lowercase hex).
    pub id: String,
}

impl Item {
    /// Build an item from a timestamp and dedup id.
    pub fn new(timestamp: i64, id: impl Into<String>) -> Self {
        Self {
            timestamp,
            id: id.into(),
        }
    }

    /// The `(timestamp, id)` sort key.
    fn key(&self) -> (i64, &str) {
        (self.timestamp, self.id.as_str())
    }
}

/// Sort a set of items into the canonical `(timestamp, id)` order and drop exact
/// duplicates, yielding the shape both peers must agree on before reconciling.
pub fn sorted(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by(|a, b| a.key().cmp(&b.key()));
    items.dedup();
    items
}

/// A half-open bound over the `(timestamp, id)` order. `None` means unbounded
/// (the very start or very end). Inclusive lower, exclusive upper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// Lower key (inclusive). `None` = -infinity.
    pub lo: Option<(i64, String)>,
    /// Upper key (exclusive). `None` = +infinity.
    pub hi: Option<(i64, String)>,
}

impl Bound {
    /// The full range covering every item.
    pub fn full() -> Self {
        Self { lo: None, hi: None }
    }
}

/// A description of one range plus the peer's claim about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RangeMsg {
    /// "Over this range my ids digest to `fp`." The cheap path.
    Fingerprint {
        /// The range these ids fall in.
        range: Bound,
        /// `fingerprint` of the sender's ids in `range`.
        fp: String,
    },
    /// "Over this range my exact ids are `ids`." Sent for small ranges so the
    /// peer can diff directly (NIP-77 IdList mode).
    IdList {
        /// The range these ids fall in.
        range: Bound,
        /// The sender's ids in `range`, in canonical order.
        ids: Vec<String>,
    },
}

impl RangeMsg {
    /// The range this message describes.
    pub fn range(&self) -> &Bound {
        match self {
            RangeMsg::Fingerprint { range, .. } | RangeMsg::IdList { range, .. } => range,
        }
    }
}

/// One round of the protocol: a batch of per-range claims.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// The per-range claims in this round.
    pub ranges: Vec<RangeMsg>,
}

impl Message {
    /// Whether this message settles everything (no claims left to resolve).
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Total number of dedup ids carried inline by this message (its on-the-wire
    /// item cost). Fingerprint claims carry none; id-list claims carry their ids.
    /// Lets a caller assert the O(diff) property by bytes/items transferred.
    pub fn id_count(&self) -> usize {
        self.ranges
            .iter()
            .map(|m| match m {
                RangeMsg::Fingerprint { .. } => 0,
                RangeMsg::IdList { ids, .. } => ids.len(),
            })
            .sum()
    }
}

/// The fingerprint of an ordered id slice: `SHA-256(count_le || id_bytes...)`.
///
/// Prefixing the count means a range and a strict superset of it never share a
/// fingerprint even in the degenerate empty case, and the empty range has a
/// fixed, well-defined value.
pub fn fingerprint(ids: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update((ids.len() as u64).to_le_bytes());
    for id in ids {
        hasher.update(id.as_bytes());
    }
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for b in digest {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// The items of a sorted set that fall within `bound`, as a slice view.
fn items_in<'a>(sorted: &'a [Item], bound: &Bound) -> &'a [Item] {
    // `sorted` is in `(timestamp, id)` order, so the in-range items are a
    // contiguous slice; find its edges by binary search on the key.
    let start = match &bound.lo {
        None => 0,
        Some((t, id)) => sorted.partition_point(|it| (it.timestamp, it.id.as_str()) < (*t, id)),
    };
    let end = match &bound.hi {
        None => sorted.len(),
        Some((t, id)) => sorted.partition_point(|it| (it.timestamp, it.id.as_str()) < (*t, id)),
    };
    &sorted[start..end.max(start)]
}

/// Build a fingerprint-or-idlist claim for a sorted slice over `bound`.
fn claim_for(slice: &[Item], bound: Bound) -> RangeMsg {
    if slice.len() <= ID_LIST_THRESHOLD {
        RangeMsg::IdList {
            range: bound,
            ids: slice.iter().map(|it| it.id.clone()).collect(),
        }
    } else {
        let ids: Vec<&str> = slice.iter().map(|it| it.id.as_str()).collect();
        RangeMsg::Fingerprint {
            range: bound,
            fp: fingerprint(&ids),
        }
    }
}

/// Split a sorted slice's covering `bound` into up to [`BUCKETS`] sub-ranges
/// (each a claim) so a mismatch can recurse with O(log) depth. Splits are at
/// item boundaries, so each sub-range's lower bound is a real item key.
fn split(slice: &[Item], bound: &Bound) -> Vec<RangeMsg> {
    if slice.is_empty() {
        // Nothing here on our side; assert the empty range so the peer can
        // surface ids it holds that we lack.
        return vec![RangeMsg::IdList {
            range: bound.clone(),
            ids: Vec::new(),
        }];
    }
    let n = slice.len();
    let chunk = n.div_ceil(BUCKETS).max(1);
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let j = (i + chunk).min(n);
        // Lower bound = this chunk's first item key (or inherit the parent's lo
        // for the first chunk so we don't drop items exactly on the edge).
        let lo = if i == 0 {
            bound.lo.clone()
        } else {
            Some((slice[i].timestamp, slice[i].id.clone()))
        };
        // Upper bound = next chunk's first item key (exclusive), or the parent's
        // hi for the last chunk.
        let hi = if j == n {
            bound.hi.clone()
        } else {
            Some((slice[j].timestamp, slice[j].id.clone()))
        };
        out.push(claim_for(&slice[i..j], Bound { lo, hi }));
        i = j;
    }
    out
}

/// The initiator's opening message: a single full-range claim over its set.
///
/// One round-trip then expands exactly the differing regions.
pub fn initiate(local_sorted: &[Item]) -> Message {
    Message {
        ranges: vec![claim_for(local_sorted, Bound::full())],
    }
}

/// The responder step: given the initiator's `incoming` claims and the
/// responder's own sorted set, produce the reply message. Pure and stateless —
/// the responder reads only its set, so the same function serves the server
/// handler and in-process tests.
pub fn respond(local_sorted: &[Item], incoming: &Message) -> Message {
    let mut out = Vec::new();
    for msg in &incoming.ranges {
        let bound = msg.range().clone();
        let slice = items_in(local_sorted, &bound);
        match msg {
            RangeMsg::Fingerprint { fp, .. } => {
                let mine: Vec<&str> = slice.iter().map(|it| it.id.as_str()).collect();
                if fingerprint(&mine) == *fp {
                    // Agreement — drop the range entirely.
                    continue;
                }
                // Disagreement: recurse by splitting our view of the range.
                out.extend(split(slice, &bound));
            }
            RangeMsg::IdList { .. } => {
                // The peer already gave us its exact ids; answer with ours so it
                // can diff. (Always an IdList — these ranges are small.)
                out.push(RangeMsg::IdList {
                    range: bound,
                    ids: slice.iter().map(|it| it.id.clone()).collect(),
                });
            }
        }
    }
    Message { ranges: out }
}

/// What the initiator extracts from a responder reply over one round.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconcile {
    /// Ids only the initiator holds (the peer needs these from us).
    pub have: Vec<String>,
    /// Ids only the responder holds (we need these from the peer).
    pub need: Vec<String>,
    /// The next message to send; empty when reconciliation has converged.
    pub next: Message,
}

/// Process a responder `reply` against the initiator's own sorted set: settle
/// every `IdList` range into `have`/`need`, and re-pose every `Fingerprint`
/// range (compare, and split on mismatch) for the next round.
pub fn reconcile(local_sorted: &[Item], reply: &Message) -> Reconcile {
    use std::collections::BTreeSet;
    let mut out = Reconcile::default();
    let mut next = Vec::new();
    for msg in &reply.ranges {
        let bound = msg.range().clone();
        let slice = items_in(local_sorted, &bound);
        match msg {
            RangeMsg::IdList { ids: theirs, .. } => {
                let mine: BTreeSet<&str> = slice.iter().map(|it| it.id.as_str()).collect();
                let theirs_set: BTreeSet<&str> = theirs.iter().map(|s| s.as_str()).collect();
                for id in mine.difference(&theirs_set) {
                    out.have.push((*id).to_string());
                }
                for id in theirs_set.difference(&mine) {
                    out.need.push((*id).to_string());
                }
            }
            RangeMsg::Fingerprint { fp, .. } => {
                let mine: Vec<&str> = slice.iter().map(|it| it.id.as_str()).collect();
                if fingerprint(&mine) == *fp {
                    continue;
                }
                // Re-pose this still-differing range, split on our side.
                next.extend(split(slice, &bound));
            }
        }
    }
    out.next = Message { ranges: next };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ts: i64, id: &str) -> Item {
        Item::new(ts, id)
    }

    /// Drive the full client/server loop in-process and return (have, need,
    /// rounds).
    fn run(client: Vec<Item>, server: Vec<Item>) -> (Vec<String>, Vec<String>, usize) {
        let client = sorted(client);
        let server = sorted(server);
        let mut msg = initiate(&client);
        let mut have = Vec::new();
        let mut need = Vec::new();
        let mut rounds = 0;
        loop {
            rounds += 1;
            assert!(rounds < 100, "must converge");
            let reply = respond(&server, &msg);
            let rec = reconcile(&client, &reply);
            have.extend(rec.have);
            need.extend(rec.need);
            if rec.next.is_empty() {
                break;
            }
            msg = rec.next;
        }
        have.sort();
        have.dedup();
        need.sort();
        need.dedup();
        (have, need, rounds)
    }

    #[test]
    fn identical_sets_transfer_nothing() {
        let items: Vec<Item> = (0..100).map(|i| item(i, &format!("id{i:03}"))).collect();
        let (have, need, _rounds) = run(items.clone(), items);
        assert!(have.is_empty());
        assert!(need.is_empty());
    }

    #[test]
    fn detects_single_backdated_item_on_server() {
        let mut server: Vec<Item> = (0..100).map(|i| item(i, &format!("id{i:03}"))).collect();
        let client = server.clone();
        // A backdated item appears on the server (low timestamp).
        server.push(item(-5, "backdated"));
        let (have, need, _rounds) = run(client, server);
        assert!(have.is_empty());
        assert_eq!(need, vec!["backdated".to_string()]);
    }

    #[test]
    fn detects_handful_of_differences_both_directions() {
        let base: Vec<Item> = (0..200).map(|i| item(i, &format!("id{i:03}"))).collect();
        let mut client = base.clone();
        let mut server = base.clone();
        client.push(item(1, "only_client_a"));
        client.push(item(50, "only_client_b"));
        server.push(item(2, "only_server_a"));
        server.push(item(150, "only_server_b"));
        server.push(item(-1, "only_server_back"));
        let (have, need, _rounds) = run(client, server);
        assert_eq!(have, vec!["only_client_a", "only_client_b"]);
        assert_eq!(
            need,
            vec!["only_server_a", "only_server_b", "only_server_back"]
        );
    }

    #[test]
    fn empty_client_needs_everything() {
        let server: Vec<Item> = (0..30).map(|i| item(i, &format!("id{i:03}"))).collect();
        let (have, need, _rounds) = run(Vec::new(), server.clone());
        assert!(have.is_empty());
        assert_eq!(need.len(), server.len());
    }

    #[test]
    fn convergence_is_logarithmic_not_linear() {
        // 4096 identical items + one difference must converge in few rounds,
        // proving the recursion depth scales with log(N), not N.
        let base: Vec<Item> = (0..4096).map(|i| item(i, &format!("id{i:04}"))).collect();
        let mut server = base.clone();
        server.push(item(123, "needle"));
        let (_have, need, rounds) = run(base, server);
        assert_eq!(need, vec!["needle"]);
        // log16(4096) = 3, plus opening and settling rounds — comfortably < 10.
        assert!(rounds < 10, "took {rounds} rounds, expected logarithmic");
    }

    #[test]
    fn message_round_trips_as_json() {
        let m = initiate(&sorted(vec![item(1, "a"), item(2, "b")]));
        let s = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }
}
