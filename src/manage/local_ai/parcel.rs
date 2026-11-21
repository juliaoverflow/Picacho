//! Tracking number confirmation, with Vast Parcel: a number that has a
//! carrier's format and check digit (see `manage::parcels`) can still be an
//! order, invoice or phone number. About one random 10- or 12-digit number
//! in ten passes some carrier's check. The model decides from the words
//! around each one whether the message gives it as the tracking number of a
//! parcel.
//!
//! The features are the sign-in code model's (`otp::Text::features`) and
//! the parcel concepts the words around the number name (`lexicon.json`:
//! track, parcel, deliver, ship, and order, invoice, customer, phone,
//! reference), built exactly as `models/parcel/features.py` builds them for
//! training; `models/parcel/parity.json` checks the two agree.
use super::otp::{self, CONTEXT, NEAR, SUBJECT_WORDS, Text, WINDOW, vector, words};
use super::runtime::Scorer;
use std::sync::LazyLock;

/// The hashed feature space, the same as the sign-in code model's.
pub const DIM: usize = otp::DIM;
/// How far before the number the close window reaches, in characters.
const CLOSE: usize = 60;

/// Concepts for parcels and for the other numbers mail gives, each with
/// word stems in many languages; "=" marks a stem that must be the whole
/// word and "^" one the word must start with. The model was trained with
/// this same file.
static LEXICON: LazyLock<Vec<(String, Vec<String>)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../models/parcel/lexicon.json")).expect("lexicon")
});

/// The parcel concepts named by any of the words, in lexicon order.
pub(crate) fn concepts(words: &[String]) -> Vec<&'static str> {
    LEXICON
        .iter()
        .filter(|(_, stems)| {
            stems.iter().any(|st| {
                if let Some(whole) = st.strip_prefix('=') {
                    words.iter().any(|w| w == whole)
                } else if let Some(start) = st.strip_prefix('^') {
                    words.iter().any(|w| w.starts_with(start))
                } else {
                    words.iter().any(|w| w.contains(st.as_str()))
                }
            })
        })
        .map(|(name, _)| name.as_str())
        .collect()
}

/// The feature strings of the number at `start..end` (in characters of
/// `text`).
pub(crate) fn features(text: &Text, subject: &str, start: usize, end: usize) -> Vec<String> {
    let mut f = text.features(subject, start, end);
    let left = text.slice(start.saturating_sub(WINDOW), start);
    let right = text.slice(end, end + WINDOW);
    let lw = words(&left, true);
    let lw = &lw[lw.len().saturating_sub(CONTEXT)..];
    let rw = words(&right, true);
    let rw = &rw[..rw.len().min(CONTEXT)];
    let sw = words(subject, true);
    let sw = &sw[..sw.len().min(SUBJECT_WORDS)];
    for (region, ws) in [
        ("QL", lw),
        ("QR", rw),
        ("QNL", &lw[lw.len().saturating_sub(NEAR)..]),
        ("QNR", &rw[..rw.len().min(NEAR)]),
        ("QS", sw),
    ] {
        for name in concepts(ws) {
            f.push(format!("{region}:{name}"));
        }
    }
    // Close before the number on its line, and close before it at all, with
    // words in scripts written without spaces kept whole.
    let line_start = text.chars[..start.min(text.chars.len())]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1);
    let line_left = text.slice(line_start, start);
    let close = text.slice(start.saturating_sub(CLOSE), start);
    for (region, t) in [
        ("QLL", line_left.as_str()),
        ("QWL", close.as_str()),
        ("QWS", subject),
    ] {
        for name in concepts(&words(t, false)) {
            f.push(format!("{region}:{name}"));
        }
    }
    f
}

/// Scores tracking number candidates with a loaded model.
pub struct Confirmer {
    scorer: Box<dyn Scorer>,
    pub threshold: f32,
}

impl Confirmer {
    pub fn new(scorer: Box<dyn Scorer>, threshold: f32) -> Self {
        Self { scorer, threshold }
    }

    /// The model's probability that the candidate at `start..end` (in
    /// characters of `text`) is the message's tracking number. None if the
    /// model couldn't score it.
    pub(crate) fn score(
        &self,
        text: &Text,
        subject: &str,
        start: usize,
        end: usize,
    ) -> Option<f32> {
        let x = vector(&features(text, subject, start, end));
        self.scorer.score(&x).ok()
    }

    /// Whether the candidate is a tracking number.
    pub(crate) fn confirms(&self, text: &Text, subject: &str, start: usize, end: usize) -> bool {
        self.score(text, subject, start, end)
            .is_some_and(|p| p >= self.threshold)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::manage::local_ai::runtime::NativeModel;

    pub(crate) fn confirmer() -> Confirmer {
        let model = NativeModel::load(&super::super::test_model(super::super::PARCEL.file))
            .expect("the published parcel model");
        Confirmer::new(Box::new(model), super::super::PARCEL.threshold)
    }

    /// The candidates, features, vectors and scores the training code
    /// computed for its examples come out the same here.
    #[test]
    fn matches_the_training_code() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/parcel/parity.json");
        let cases: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let confirmer = confirmer();
        for case in cases.as_array().unwrap() {
            let subject = case["subject"].as_str().unwrap();
            let body = case["body"].as_str().unwrap();
            let text = Text::new(subject, body);
            let found: Vec<(usize, usize)> = crate::manage::parcels::candidates(&text)
                .into_iter()
                .map(|c| (c.start, c.end))
                .collect();
            for w in case["candidates"].as_array().unwrap() {
                let (s, e) = (
                    w["start"].as_u64().unwrap() as usize,
                    w["end"].as_u64().unwrap() as usize,
                );
                assert!(
                    found.contains(&(s, e)),
                    "{:?} isn't found as a candidate in {subject:?}: {found:?}",
                    w["number"]
                );
                let features = super::features(&text, subject, s, e);
                let expected: Vec<String> = w["features"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|f| f.as_str().unwrap().to_owned())
                    .collect();
                assert_eq!(features, expected, "features of {:?}", w["number"]);
                let x = vector(&features);
                let expected: Vec<(usize, f32)> = w["vector"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| {
                        (
                            p[0].as_u64().unwrap() as usize,
                            p[1].as_f64().unwrap() as f32,
                        )
                    })
                    .collect();
                assert_eq!(x.len(), expected.len());
                for ((i, v), (j, u)) in x.iter().zip(&expected) {
                    assert_eq!(i, j);
                    assert!((v - u).abs() < 1e-5, "{v} vs {u}");
                }
                let score = confirmer.score(&text, subject, s, e).unwrap();
                let want = w["score"].as_f64().unwrap() as f32;
                assert!(
                    (score - want).abs() < 1e-4,
                    "{:?}: {score} vs {want}",
                    w["number"]
                );
            }
        }
    }
}
