//! Contribution kinds and feedback aggregation.
//!
//! Thin, pure orchestration helpers over the protocol model: what the user can
//! contribute ([`Contribution`]) and how a target's annotations roll up into
//! the Feedback screen ([`aggregate`] → [`FeedbackView`]). The typed feedback
//! itself stays in the annotation BODY (INVARIANT 2) — nothing here invents
//! vocabulary.

use freedback_protocol::{Annotation, Body, Motivation};
use serde::{Deserialize, Serialize};

/// The default license contributions are published under (data licensing,
/// ADR 0022).
pub const DEFAULT_LICENSE: &str = "https://creativecommons.org/licenses/by/4.0/";

/// What the user contributes from the composer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Contribution {
    /// A star rating on the default 1..=5 scale.
    Stars { value: f64 },
    /// Thumbs up / down.
    Thumb { up: bool },
    /// A free-text comment.
    Comment { text: String },
    /// A single tag.
    Tag { text: String },
    /// An issue / problem report (`oa:editing`, ADR 0023).
    Issue { text: String },
}

impl Contribution {
    /// The journal `kind` name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Contribution::Stars { .. } => "stars",
            Contribution::Thumb { .. } => "thumb",
            Contribution::Comment { .. } => "comment",
            Contribution::Tag { .. } => "tag",
            Contribution::Issue { .. } => "issue",
        }
    }

    /// A short human summary for the journal row.
    pub fn summary(&self) -> String {
        match self {
            Contribution::Stars { value } => format!("★ {value}"),
            Contribution::Thumb { up: true } => "👍".to_string(),
            Contribution::Thumb { up: false } => "👎".to_string(),
            Contribution::Comment { text }
            | Contribution::Tag { text }
            | Contribution::Issue { text } => truncate(text, 80),
        }
    }

    /// The annotation motivation for this kind.
    pub fn motivation(&self) -> Motivation {
        match self {
            Contribution::Stars { .. } | Contribution::Thumb { .. } => Motivation::Assessing,
            Contribution::Comment { .. } => Motivation::Commenting,
            Contribution::Tag { .. } => Motivation::Tagging,
            Contribution::Issue { .. } => Motivation::Editing,
        }
    }

    /// The typed annotation body for this kind.
    pub fn body(&self) -> Body {
        match self {
            Contribution::Stars { value } => Body::star(*value),
            Contribution::Thumb { up } => Body::thumb(*up),
            Contribution::Comment { text } => Body::Comment {
                value: text.clone(),
            },
            Contribution::Tag { text } => Body::Tag {
                value: text.clone(),
            },
            Contribution::Issue { text } => Body::issue(text.clone()),
        }
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// A textual item (comment or tag) in the feedback view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextItem {
    pub text: String,
    /// The issuer id, when the annotation was signed.
    pub creator: Option<String>,
    /// RFC 3339 `created`, when present.
    pub created: Option<String>,
}

/// The aggregated feedback for one target — the Feedback screen's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FeedbackView {
    /// The target these aggregates are about.
    pub target: String,
    /// Mean star rating (1..=5 scale), when any exist.
    pub star_avg: Option<f64>,
    /// How many star ratings went into the average.
    pub star_count: usize,
    /// Thumb tally.
    pub thumbs_up: usize,
    pub thumbs_down: usize,
    /// Comments, oldest first.
    pub comments: Vec<TextItem>,
    /// Tags, oldest first.
    pub tags: Vec<TextItem>,
    /// Issues / problem reports (`Body::Issue`, ADR 0023), oldest first.
    pub issues: Vec<TextItem>,
    /// Total number of annotations considered.
    pub total: usize,
}

/// Roll a target's annotations up into a [`FeedbackView`]. Pure — the fetch
/// happens in [`crate::AppCore::get_feedback`]. Annotations for other targets
/// are ignored defensively.
pub fn aggregate(target: &str, annotations: &[Annotation]) -> FeedbackView {
    let mut view = FeedbackView {
        target: target.to_string(),
        ..FeedbackView::default()
    };
    let mut star_sum = 0.0;

    let mut anns: Vec<&Annotation> = annotations
        .iter()
        .filter(|a| a.target.source() == target)
        .collect();
    // Oldest first, so comment/tag lists read chronologically.
    anns.sort_by(|a, b| a.created.cmp(&b.created));

    for ann in anns {
        view.total += 1;
        let creator = ann.creator.as_ref().map(|c| c.id.clone());
        for body in &ann.body {
            match body {
                Body::StarRating { value, .. } => {
                    star_sum += value;
                    view.star_count += 1;
                }
                Body::ThumbRating { up: true } => view.thumbs_up += 1,
                Body::ThumbRating { up: false } => view.thumbs_down += 1,
                Body::Comment { value } => view.comments.push(TextItem {
                    text: value.clone(),
                    creator: creator.clone(),
                    created: ann.created.clone(),
                }),
                Body::Tag { value } => view.tags.push(TextItem {
                    text: value.clone(),
                    creator: creator.clone(),
                    created: ann.created.clone(),
                }),
                Body::Issue { value } => view.issues.push(TextItem {
                    text: value.clone(),
                    creator: creator.clone(),
                    created: ann.created.clone(),
                }),
                // Scalar ratings have no widget in the app yet; they still
                // count toward `total` via the annotation itself.
                Body::ScalarRating { .. } => {}
            }
        }
    }

    if view.star_count > 0 {
        view.star_avg = Some(star_sum / view.star_count as f64);
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;
    use freedback_protocol::{Creator, Target};

    const TARGET: &str = "https://id.gs1.org/01/03017620422003";

    fn ann(body: Body, motivation: Motivation, created: &str) -> Annotation {
        Annotation::new(motivation, Target::Iri(TARGET.into()), vec![body])
            .with_created(created)
            .with_creator(Creator::new("urn:freedback:key:test"))
    }

    #[test]
    fn empty_aggregate_is_all_zeroes() {
        let view = aggregate(TARGET, &[]);
        assert_eq!(view.star_avg, None);
        assert_eq!(view.star_count, 0);
        assert_eq!(view.thumbs_up + view.thumbs_down, 0);
        assert!(view.comments.is_empty() && view.tags.is_empty());
        assert_eq!(view.total, 0);
    }

    #[test]
    fn star_average_and_count() {
        let anns = vec![
            ann(
                Body::star(4.0),
                Motivation::Assessing,
                "2026-07-01T10:00:00Z",
            ),
            ann(
                Body::star(5.0),
                Motivation::Assessing,
                "2026-07-01T11:00:00Z",
            ),
            ann(
                Body::star(3.0),
                Motivation::Assessing,
                "2026-07-01T12:00:00Z",
            ),
        ];
        let view = aggregate(TARGET, &anns);
        assert_eq!(view.star_avg, Some(4.0));
        assert_eq!(view.star_count, 3);
        assert_eq!(view.total, 3);
    }

    #[test]
    fn thumb_tally() {
        let anns = vec![
            ann(
                Body::thumb(true),
                Motivation::Assessing,
                "2026-07-01T10:00:00Z",
            ),
            ann(
                Body::thumb(true),
                Motivation::Assessing,
                "2026-07-01T11:00:00Z",
            ),
            ann(
                Body::thumb(false),
                Motivation::Assessing,
                "2026-07-01T12:00:00Z",
            ),
        ];
        let view = aggregate(TARGET, &anns);
        assert_eq!((view.thumbs_up, view.thumbs_down), (2, 1));
    }

    #[test]
    fn comments_and_tags_are_chronological() {
        let anns = vec![
            ann(
                Body::Comment {
                    value: "second".into(),
                },
                Motivation::Commenting,
                "2026-07-02T10:00:00Z",
            ),
            ann(
                Body::Comment {
                    value: "first".into(),
                },
                Motivation::Commenting,
                "2026-07-01T10:00:00Z",
            ),
            ann(
                Body::Tag {
                    value: "vegan".into(),
                },
                Motivation::Tagging,
                "2026-07-03T10:00:00Z",
            ),
        ];
        let view = aggregate(TARGET, &anns);
        let texts: Vec<_> = view.comments.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "second"]);
        assert_eq!(view.tags[0].text, "vegan");
        assert_eq!(view.total, 3);
    }

    #[test]
    fn other_targets_are_ignored() {
        let other = Annotation::new(
            Motivation::Assessing,
            Target::Iri("https://example.com/other".into()),
            vec![Body::star(1.0)],
        )
        .with_created("2026-07-01T10:00:00Z");
        let view = aggregate(TARGET, &[other]);
        assert_eq!(view.total, 0);
        assert_eq!(view.star_avg, None);
    }

    #[test]
    fn contribution_mapping_is_faithful() {
        assert_eq!(Contribution::Stars { value: 4.0 }.body(), Body::star(4.0));
        assert_eq!(Contribution::Thumb { up: true }.body(), Body::thumb(true));
        assert_eq!(
            Contribution::Comment { text: "hi".into() }.motivation(),
            Motivation::Commenting
        );
        assert_eq!(
            Contribution::Tag { text: "t".into() }.motivation(),
            Motivation::Tagging
        );
        assert_eq!(Contribution::Stars { value: 4.0 }.kind_name(), "stars");
        assert_eq!(
            Contribution::Issue {
                text: "broken link".into()
            }
            .body(),
            Body::issue("broken link")
        );
        assert_eq!(
            Contribution::Issue { text: "x".into() }.motivation(),
            Motivation::Editing
        );
        assert_eq!(
            Contribution::Issue { text: "x".into() }.kind_name(),
            "issue"
        );
    }

    #[test]
    fn summaries_are_short() {
        let long = "x".repeat(300);
        let summary = Contribution::Comment { text: long }.summary();
        assert!(summary.chars().count() <= 80);
        assert!(summary.ends_with('…'));
        assert_eq!(Contribution::Stars { value: 4.5 }.summary(), "★ 4.5");
    }

    #[test]
    fn contribution_json_shape_is_stable_for_the_ui() {
        // The UI sends contributions as tagged JSON — pin the shape.
        let c: Contribution = serde_json::from_str(r#"{"kind":"stars","value":4}"#).unwrap();
        assert_eq!(c, Contribution::Stars { value: 4.0 });
        let c: Contribution = serde_json::from_str(r#"{"kind":"comment","text":"hello"}"#).unwrap();
        assert_eq!(
            c,
            Contribution::Comment {
                text: "hello".into()
            }
        );
        let c: Contribution =
            serde_json::from_str(r#"{"kind":"issue","text":"broken link"}"#).unwrap();
        assert_eq!(
            c,
            Contribution::Issue {
                text: "broken link".into()
            }
        );
    }

    #[test]
    fn issues_are_chronological_and_aggregated() {
        let anns = vec![
            ann(
                Body::issue("second issue"),
                Motivation::Editing,
                "2026-07-02T10:00:00Z",
            ),
            ann(
                Body::issue("first issue"),
                Motivation::Editing,
                "2026-07-01T10:00:00Z",
            ),
        ];
        let view = aggregate(TARGET, &anns);
        let texts: Vec<_> = view.issues.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, vec!["first issue", "second issue"]);
        assert_eq!(view.total, 2);
    }
}
