//! Trigger recognition for `sift should-search`.
//!
//! The model reading a user message is the real classifier — telling S0 from S2
//! is semantics, not regex. This module supplies that decision with *evidence*
//! and keeps the common case cheap. It does not override the model.
//!
//! ## Fail-open is a hard requirement
//!
//! Absence of a signal must never suppress a search. A trigger detector that
//! made recall worse than having no detector at all would be worthless:
//!
//! * no signal          -> S0, `hard = false`  (permitted, merely not recommended)
//! * suppression needs *positive* evidence: a negation or a quotation
//! * empty input        -> S2, `hard = false`  (fail open)
//!
//! `hard = true` means there is concrete proof the message is not a recall
//! request. Only that suppresses.
//!
//! ## Why these words
//!
//! Corpus counts in sift.db, measured 2026-10-08 over 139.790 docs. Counts are
//! **phrase-adjacent** (`MATCH '"wie haben wir"'`), not AND-anywhere: the
//! difference is large — "wie haben wir" adjacent is 3, AND-anywhere 823 — and
//! an incoming user message contains the phrase adjacently, so adjacency is the
//! honest proxy for precision.
//!
//! | tier A (one hit is enough) | hits |   | tier B (two, or one + deficit) | hits |
//! |---|---|---|---|---|
//! | wie haben wir   |   3 |   | gelöst      | 214 |
//! | beschlossen     |   8 |   | workaround  | 355 |
//! | festgelegt      |  36 |   | entschied*  | 418 |
//! | schonmal        |  35 |   | remember    | 861 |
//! | keine ahnung    |  40 |   | erneut      | 996 |
//! | zuletzt         | 227 |   | früher      | 933 |
//! | vorhin          | 218 |   | kaputt      | 898 |
//! | damals          | 197 |   | previously  | 312 |
//! | erinner*        | 629 |   | nochmal     | 776 |
//!
//! `recall` (4.391) is deliberately absent: despite being the domain term it is
//! far too common to discriminate. `nochmal` (776) is tier B because in German
//! it usually means "once more", not "as we did back then".

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum State {
    /// Current context is enough. Do not search.
    S0,
    /// Refers to the past, but git log / ripgrep are cheaper and more authoritative.
    S1,
    /// Session history is the only possible source. Search.
    S2,
    /// Explicit retrieval request: "which sessions touched X". Different query shape.
    S3,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::S0 => "S0",
            State::S1 => "S1",
            State::S2 => "S2",
            State::S3 => "S3",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Verdict {
    pub state: State,
    /// True only when suppression is certain (negation or quotation detected).
    pub hard: bool,
    /// Human-readable evidence. Never empty: a verdict without a reason is a
    /// verdict nobody can audit.
    pub why: Vec<String>,
    /// Terms worth searching for, extracted from the message.
    pub terms: Vec<String>,
    /// True when the message mentions sift or search explicitly.
    pub explicit: bool,
}

/// Adjacent, all words exact.
const TIER_A: &[&[&str]] = &[
    &["wie", "haben", "wir"],
    &["was", "haben", "wir"],
    &["was", "machen", "wir"],
    &["beschlossen"],
    &["festgelegt"],
    &["schonmal"],
    &["zuletzt"],
    &["vorhin"],
    &["damals"],
];

/// Adjacent, final word may match as a prefix ("keine ahnung" -> "keine ahnungen").
///
/// ["wie","wir"] is the loosest tier-A entry at 98 corpus hits — still 40x below
/// the noise floor ("wieder" 3.711). It earns its place because German is
/// verb-final: "wie haben wir X gelöst" and "wie wir X gelöst haben" are equally
/// natural, and adjacency alone would miss the second one entirely.
const TIER_A_STEM: &[&[&str]] = &[&["keine", "ahnung"], &["kein", "plan"], &["wie", "wir"]];

/// Single token matching as a prefix: erinner* covers erinnert/erinnerte/erinnerung.
const TIER_A_PREFIX: &[&str] = &["erinner"];

const TIER_B: &[&[&str]] = &[
    &["workaround"],
    &["gelöst"],
    &["erneut"],
    &["früher"],
    &["kaputt"],
    &["nochmal"],
];

const TIER_B_STEM: &[&[&str]] = &[
    &["remember"],
    &["previous"],
    &["last", "time"],
    &["letzte", "mal"],
    &["letztes", "mal"],
    &["letzten", "mal"],
];

const TIER_B_PREFIX: &[&str] = &["entschied"];

/// Paired with a single tier-B hit to reach S2.
const DEFICIT: &[&[&str]] = &[
    &["fehlt"],
    &["geht", "nicht"],
    &["funktioniert", "nicht"],
    &["nicht", "gefunden"],
    &["unknown", "error"],
];

/// Final word stems, so session/sessions and project/projects both match.
const RETRIEVAL: &[&[&str]] = &[
    &["welche", "session"],
    &["welchen", "session"],
    &["alle", "session"],
    &["in", "welchen", "session"],
    &["finde", "alle"],
    &["wann", "haben", "wir"],
    &["welche", "project"],
    &["welches", "project"],
];

/// Triggers after these are about producing text, not about recalling it.
/// Deliberately NOT "sag": "sag mir, wie wir das gelöst haben" is a recall
/// request with a reporting verb. The object decides, not the verb, and the
/// quotation check already covers the real writing cases.
const WRITE_VERBS: &[&[&str]] = &[&["schreib"], &["formulier"], &["dokumentier"]];

const NEGATIONS: &[&str] = &["nicht", "kein", "keine", "nichts", "ohne", "niemals", "nie"];

/// Words that name the tool rather than the subject. Searching for "sift"
/// returns this project's own documentation, which is circular.
const TOOL_VOCAB: &[&str] = &["sift", "snippet", "snippets", "search"];

/// Dropped from extracted terms: too common to narrow anything down.
const STOPWORDS: &[&str] = &[
    "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen", "einem", "einer", "und",
    "oder", "aber", "auch", "als", "am", "an", "auf", "aus", "bei", "bin", "bis", "bist", "da",
    "dann", "dass", "dein", "deine", "dich", "dir", "du", "es", "für", "hat", "hatte", "hatten",
    "hier", "ich", "ihr", "im", "in", "ist", "ja", "jetzt", "kann", "kein", "können", "mal",
    "man", "mein", "meine", "mir", "mit", "nach", "nicht", "noch", "nun", "nur", "ob", "ohne",
    "schon", "sehr", "sein", "seine", "sich", "sie", "sind", "so", "über", "um", "uns", "unser",
    "vom", "von", "vor", "war", "waren", "was", "weg", "weil", "wenn", "werden", "wie", "wieder",
    "wir", "wird", "wo", "wollen", "würde", "zu", "zum", "zur",
    // High-frequency verbs and participles. These never narrow a search, and
    // because the search sanitizer adds a prefix star they would otherwise
    // become broad noise terms.
    "sag", "sagt", "sagte", "sagen", "gesagt", "nutze", "nutzt", "nutz", "kannst", "konnte",
    "sollte", "soll", "sollst", "muss", "musst", "müssen", "will", "willst", "brauch",
    "braucht", "gemacht", "machen", "geht", "gehen", "gegeben",
    "the", "and", "for", "with", "that", "this", "you", "your", "was", "were", "have", "has",
    "had", "did", "does", "what", "when", "where", "which", "how", "can", "could", "should",
    "would", "there", "their", "them", "then", "than", "about", "into", "from",
];

#[derive(Debug, Clone)]
struct Tok {
    text: String,
    /// The token sat inside a quoted span.
    quoted: bool,
}

/// Split on non-alphanumerics, remembering which tokens were inside quotes.
fn tokenize(input: &str) -> Vec<Tok> {
    let is_quote = |c: char| matches!(c, '"' | '\'' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}');

    let mut out: Vec<Tok> = Vec::new();
    let mut word = String::new();
    let mut word_quoted = false;
    let mut quoted = false;

    let flush = |word: &mut String, quoted: bool, out: &mut Vec<Tok>| {
        if !word.is_empty() {
            out.push(Tok {
                text: std::mem::take(word),
                quoted,
            });
        }
    };

    for c in input.chars() {
        if is_quote(c) {
            flush(&mut word, word_quoted, &mut out);
            quoted = !quoted;
            word_quoted = quoted;
        } else if c.is_alphanumeric() {
            if word.is_empty() {
                word_quoted = quoted;
            }
            word.extend(c.to_lowercase());
        } else {
            flush(&mut word, word_quoted, &mut out);
        }
    }
    flush(&mut word, word_quoted, &mut out);
    out
}

/// A phrase hit: token index and length, so negation lookup can skip its own span.
struct Hit {
    start: usize,
    len: usize,
    text: String,
    quoted: bool,
}

/// Adjacent token sequence. When `stem_final` is set, the last word matches as a
/// prefix, which is how "welche session" catches "welche sessions".
fn find_phrase(toks: &[Tok], words: &[&str], stem_final: bool) -> Option<Hit> {
    if words.is_empty() || toks.len() < words.len() {
        return None;
    }
    for i in 0..=(toks.len() - words.len()) {
        let mut ok = true;
        for (k, w) in words.iter().enumerate() {
            let last = k == words.len() - 1;
            let hit = if last && stem_final {
                toks[i + k].text.starts_with(w)
            } else {
                &toks[i + k].text == w
            };
            if !hit {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(Hit {
                start: i,
                len: words.len(),
                text: words.join(" "),
                quoted: toks[i].quoted || (words.len() > 1 && toks[i..i + words.len()].iter().any(|t| t.quoted)),
            });
        }
    }
    None
}

fn find_prefix(toks: &[Tok], prefix: &str) -> Option<Hit> {
    toks.iter().position(|t| t.text.starts_with(prefix)).map(|i| Hit {
        start: i,
        len: 1,
        text: format!("{prefix}*"),
        quoted: toks[i].quoted,
    })
}

/// Positive evidence for or against a signal. Applied uniformly to every tier:
/// a trigger inside a quotation or behind a negation is not a signal no matter
/// how strong its tier.
enum Signal {
    Active(String),
    Suppressed(String),
}

fn evaluate(toks: &[Tok], hit: &Hit) -> Signal {
    if hit.quoted {
        return Signal::Suppressed("inside a quotation".to_string());
    }
    if let Some(v) = write_verb_before(toks, hit.start) {
        return Signal::Suppressed(format!("after write verb '{v}'"));
    }
    if let Some(n) = negation_near(toks, hit) {
        return Signal::Suppressed(format!("negation '{n}' within 3 tokens"));
    }
    Signal::Active(hit.text.clone())
}

fn first_hit(toks: &[Tok], phrases: &[&[&str]], stems: &[&[&str]], prefixes: &[&str]) -> Option<Hit> {
    for p in phrases {
        if let Some(h) = find_phrase(toks, p, false) {
            return Some(h);
        }
    }
    for p in stems {
        if let Some(h) = find_phrase(toks, p, true) {
            return Some(h);
        }
    }
    for p in prefixes {
        if let Some(h) = find_prefix(toks, p) {
            return Some(h);
        }
    }
    None
}

/// Negation anywhere in a +/-3 token window, excluding the phrase's own tokens:
/// German negates forward ("ich erinnere mich nicht"), English backward.
fn negation_near(toks: &[Tok], hit: &Hit) -> Option<String> {
    let lo = hit.start.saturating_sub(3);
    let hi = (hit.start + hit.len + 3).min(toks.len());
    toks[lo..hi]
        .iter()
        .enumerate()
        .filter(|(i, _)| !(hit.start..hit.start + hit.len).contains(&(lo + i)))
        .find_map(|(_, t)| {
            if NEGATIONS.contains(&t.text.as_str()) {
                Some(t.text.clone())
            } else {
                None
            }
        })
}

fn write_verb_before(toks: &[Tok], start: usize) -> Option<String> {
    (0..start)
        .rev()
        .find_map(|i| {
            if toks[i].quoted {
                return None;
            }
            WRITE_VERBS
                .iter()
                .find(|v| v.len() == 1 && toks[i].text == v[0])
                .map(|v| v[0].to_string())
        })
}

/// A file or path means the repo is the cheaper and more authoritative source
/// than a session snippet. Checked on the RAW message: normalization strips
/// `/` and `.`, which are exactly the signals being looked for.
fn mentions_path(raw: &str) -> bool {
    raw.contains('/')
        || [".md", ".ts", ".rs", ".py", ".toml", ".json", ".sql", ".sh", ".js", ".yaml", ".yml"]
            .iter()
            .any(|ext| raw.to_lowercase().contains(ext))
}

pub fn should_search(message: &str) -> Verdict {
    let toks = tokenize(message);
    let mut terms = extract_terms(message);

    if toks.is_empty() {
        return Verdict {
            state: State::S2, // fail-open
            hard: false,
            why: vec!["empty message, fail-open".into()],
            terms,
            explicit: false,
        };
    }

    let mut why: Vec<String> = Vec::new();
    let explicit = ["sift", "snippet", "snippets", "search"]
        .iter()
        .any(|w| toks.iter().any(|t| t.text == *w));
    if explicit {
        why.push("message mentions sift/search directly".into());
    }

    for r in RETRIEVAL {
        if let Some(h) = find_phrase(&toks, r, true) {
            why.push(format!("retrieval phrase '{}'", h.text));
            return Verdict {
                state: State::S3,
                hard: false,
                why,
                terms,
                explicit,
            };
        }
    }

    let mut hard = false;
    let mut tier_a: Vec<String> = Vec::new();
    let mut tier_b: Vec<String> = Vec::new();
    let mut deficit: Vec<String> = Vec::new();

    // Every tier goes through evaluate(), so a quoted or negated trigger is
    // inert regardless of how strong its tier. One hard hit should not be
    // rescued by a weaker sibling.
    for (label, hit) in [
        ("tier A", first_hit(&toks, TIER_A, TIER_A_STEM, TIER_A_PREFIX)),
        ("tier B", first_hit(&toks, TIER_B, TIER_B_STEM, TIER_B_PREFIX)),
        ("deficit", first_hit(&toks, DEFICIT, &[], &[])),
    ] {
        let Some(hit) = hit else { continue };
        match evaluate(&toks, &hit) {
            Signal::Active(text) => {
                why.push(format!("{label}: {text}"));
                match label {
                    "tier A" => tier_a.push(text),
                    "tier B" => tier_b.push(text),
                    _ => deficit.push(text),
                }
            }
            Signal::Suppressed(reason) => {
                hard = true;
                why.push(format!("{label} '{}' suppressed: {reason}", hit.text));
            }
        }
    }

    if terms.is_empty() {
        let mut fallback: Vec<String> = tier_a.clone();
        fallback.extend(tier_b.iter().cloned());
        fallback.extend(deficit.iter().cloned());
        terms = terms_fallback(&fallback);
    }

    if !tier_a.is_empty() || explicit {
        return Verdict {
            state: State::S2,
            hard,
            why,
            terms,
            explicit,
        };
    }
    if tier_b.len() >= 2 || (!tier_b.is_empty() && !deficit.is_empty()) {
        return Verdict {
            state: State::S2,
            hard,
            why,
            terms,
            explicit,
        };
    }
    if !tier_b.is_empty() {
        why.push("single tier-B hit, repo/git cheaper first".into());
        return Verdict {
            state: State::S1,
            hard,
            why,
            terms,
            explicit,
        };
    }
    if hard {
        // Positive evidence against, but nothing recommends a search either.
        return Verdict {
            state: State::S0,
            hard,
            why,
            terms,
            explicit,
        };
    }
    if mentions_path(message) {
        why.push("references a file or path, check the repo first".into());
        return Verdict {
            state: State::S1,
            hard,
            why,
            terms,
            explicit,
        };
    }

    why.push("no recall signal (fail-open: a search is still permitted)".into());
    Verdict {
        state: State::S0,
        hard,
        why,
        terms,
        explicit,
    }
}

/// Terms worth searching for. Stopwords, negations and trigger words are
/// dropped — searching for "erinnerung" narrows nothing down.
pub fn extract_terms(message: &str) -> Vec<String> {
    let mut trigger_words: Vec<&str> = Vec::new();
    for group in [TIER_A, TIER_A_STEM, TIER_B, TIER_B_STEM, DEFICIT, RETRIEVAL] {
        for phrase in group {
            trigger_words.extend_from_slice(phrase);
        }
    }
    trigger_words.extend_from_slice(TIER_A_PREFIX);
    trigger_words.extend_from_slice(TIER_B_PREFIX);

    let mut out: Vec<String> = Vec::new();
    for tok in tokenize(message) {
        if tok.quoted {
            continue;
        }
        let w = &tok.text;
        if w.chars().count() < 3 {
            continue;
        }
        if STOPWORDS.contains(&w.as_str()) || NEGATIONS.contains(&w.as_str()) {
            continue;
        }
        // The tool's own vocabulary is not a search term for itself.
        if TOOL_VOCAB.contains(&w.as_str()) {
            continue;
        }
        if trigger_words.iter().any(|t| t.starts_with(w.as_str()) || w.starts_with(t)) {
            continue;
        }
        if !out.contains(w) {
            out.push(w.clone());
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Last resort when extract_terms comes back empty but a trigger fired: the
/// trigger word itself is then the best available query material, and the
/// sanitizer will prefix-match it. Without this, "sag mir wie wir das gelöst
/// haben" yields S2 with no terms at all.
fn terms_fallback(hits: &[String]) -> Vec<String> {
    hits.iter()
        .flat_map(|h| h.split_whitespace())
        .filter(|w| w.chars().count() >= 3 && !STOPWORDS.contains(w) && !NEGATIONS.contains(w))
        .map(|w| w.to_string())
        .take(4)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_a_fires_on_history_reference() {
        for msg in [
            "erinnerst du dich wie wir den jwt cache gelöst haben?",
            "was haben wir damals mit surrealdb gemacht",
            "hast du das schonmal angefasst",
            "was haben wir da beschlossen",
            "wie haben wir das gelöst",
            "was haben wir mit der größe gemacht",
        ] {
            let v = should_search(msg);
            assert_eq!(v.state, State::S2, "{msg:?} -> {v:?}");
            assert!(!v.hard, "{msg:?} must not be a hard suppression");
        }
    }

    #[test]
    fn negation_suppresses_and_is_hard() {
        // German negates forward: "ich erinnere mich NICHT an den auth bug".
        let v = should_search("ich erinnere mich nicht an den auth bug");
        assert_eq!(v.state, State::S0, "{v:?}");
        assert!(v.hard, "negation is positive evidence, not absence of signal");
        assert!(v.why.iter().any(|w| w.contains("negation")), "{v:?}");
    }

    #[test]
    fn quotation_suppresses_and_is_hard() {
        let v = should_search("schreib mir den satz \"wie haben wir das gelöst\"");
        assert_eq!(v.state, State::S0, "{v:?}");
        assert!(v.hard, "{v:?}");
        assert!(v.why.iter().any(|w| w.contains("quotation")), "{v:?}");
    }

    #[test]
    fn explicit_retrieval_is_s3_and_stems_sessions() {
        for msg in [
            "in welchen sessions haben wir surrealdb angefasst",
            "welche sessions hatten das jwt thema",
        ] {
            let v = should_search(msg);
            assert_eq!(v.state, State::S3, "{msg:?} -> {v:?}");
        }
    }

    #[test]
    fn weak_signal_is_s1_not_s0() {
        let v = should_search("der cache ist kaputt");
        assert_eq!(v.state, State::S1, "{v:?}");
        assert!(!v.hard);
    }

    #[test]
    fn deficit_pairs_with_single_tier_b() {
        let v = should_search("das script fehlt, gelöst war es wohl mit einem workaround");
        assert!(
            matches!(v.state, State::S1 | State::S2),
            "tier B plus deficit must not silently become S0: {v:?}"
        );
    }

    #[test]
    fn path_reference_is_s1() {
        let v = should_search("schau mal in src/index.ts nach der db initialisierung");
        assert_eq!(v.state, State::S1, "{v:?}");
    }

    #[test]
    fn reporting_verb_is_not_a_write_verb() {
        // "sag mir, wie wir das gelöst haben" is a recall request, not a
        // writing task. The object decides, not the verb.
        let v = should_search("sag mir wie wir das gelöst haben");
        assert_eq!(v.state, State::S2, "{v:?}");
        assert!(!v.hard, "sag must not suppress: {v:?}");
    }

    #[test]
    fn quoted_trigger_is_inert_for_every_tier() {
        // "gelöst" sits inside the same quotation and must not rescue the
        // suppressed tier-A hit.
        let v = should_search("formulier den satz \"wie haben wir das gelöst\"");
        assert_eq!(v.state, State::S0, "{v:?}");
        assert!(v.hard, "{v:?}");
    }

    #[test]
    fn s2_always_offers_query_material() {
        // "gelöst" is itself a trigger word and was dropped from the terms; with
        // no other content word left, an S2 verdict would have nothing to search.
        let v = should_search("sag mir wie wir das gelöst haben");
        assert_eq!(v.state, State::S2, "{v:?}");
        assert!(!v.terms.is_empty(), "S2 must carry query material: {v:?}");
    }

    #[test]
    fn no_signal_is_s0_but_not_hard() {
        let v = should_search("hallo");
        assert_eq!(v.state, State::S0, "{v:?}");
        assert!(!v.hard, "absence of signal must never block a search");
    }

    #[test]
    fn empty_input_fails_open() {
        let v = should_search("");
        assert_eq!(v.state, State::S2, "{v:?}");
        assert!(!v.hard);
    }

    #[test]
    fn explicit_sift_mention_is_s2() {
        let v = should_search("nutze sift");
        assert_eq!(v.state, State::S2, "{v:?}");
        assert!(v.explicit);
    }

    #[test]
    fn terms_exclude_stopwords_and_triggers() {
        let t = extract_terms("erinnerst du dich an den jwt cache für die sessions?");
        assert!(t.contains(&"jwt".to_string()), "{t:?}");
        assert!(t.contains(&"cache".to_string()), "{t:?}");
        assert!(!t.contains(&"erinnerst".to_string()), "{t:?}");
        assert!(!t.contains(&"den".to_string()), "{t:?}");
        assert!(!t.contains(&"sessions".to_string()), "quoted/inflected noise: {t:?}");
    }

    #[test]
    fn umlauts_survive_normalisation() {
        let v = should_search("was haben wir mit der größe gemacht");
        assert_eq!(v.state, State::S2, "{v:?}");
    }
}
