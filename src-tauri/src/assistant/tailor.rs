//! Tailoring a resume to a role, without inventing anything.
//!
//! # Why the resume is reordered, not rewritten
//!
//! Asked to rewrite a resume for a job, the local model (llama3.2, 3B) did two
//! things on its first real test: it gave the user's employer the job ad's
//! title ("Senior Frontend Engineer" where the resume said "Frontend
//! Engineer"), and it silently dropped an entire previous job, the skills and
//! the name. A resume claiming a title never held is worse than no help.
//!
//! So tailoring here changes **order only**. Within each block of the resume,
//! the bullets most relevant to the job move up; on a skills line, the skills
//! the job asks for come first. Every line in the output is a line the user
//! wrote, which a test enforces: the lines in equal the lines out.
//!
//! # The guard on generated text
//!
//! The parts that are written fresh (a summary, a cover letter) are checked by
//! [`unsupported`] for two kinds of claim a small model makes up: numbers that
//! appear nowhere in the resume, and terms the job ad uses that the resume does
//! not (a "Senior" title, "GraphQL" experience). They are flagged for the user
//! to check, not silently removed: some may be fine, all deserve a look.

use std::collections::HashSet;

use serde::Serialize;

/// A resume reordered for a role.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reordered {
    pub text: String,
    /// Bullets that moved up, in their new order, so the user sees what changed.
    pub moved: Vec<String>,
    /// Words from the job found in the resume: why those bullets moved.
    pub matched: Vec<String>,
}

/// Words too common in job ads to say anything about fit.
const STOP: [&str; 58] = [
    "the", "and", "for", "with", "you", "your", "our", "are", "will", "have", "has", "who",
    "what", "this", "that", "from", "into", "about", "able", "all", "any", "can", "must",
    "nice", "plus", "work", "working", "team", "teams", "years", "year", "experience",
    "strong", "skills", "role", "job", "join", "help", "build", "building", "using", "use",
    "new", "great", "good", "including", "across", "within", "while", "more", "own", "other",
    "they", "their", "them", "not", "but", "remote",
];

fn normalise(raw: &str) -> String {
    let mapped: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '+' | '#') { c } else { ' ' })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The job's meaningful words: lowercased, three or more letters, not stop
/// words. A crude stem (trailing "s"/"ing"/"ed" dropped) lets "mentor" match
/// "mentored" and "test" match "testing".
fn keywords(text: &str) -> HashSet<String> {
    normalise(text)
        .split(' ')
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .map(stem)
        .collect()
}

fn stem(word: &str) -> String {
    for suffix in ["ing", "ed", "s"] {
        if word.len() > suffix.len() + 3 {
            if let Some(base) = word.strip_suffix(suffix) {
                return base.to_string();
            }
        }
    }
    word.to_string()
}

/// How many of the job's keywords a line mentions.
fn relevance(line: &str, job: &HashSet<String>) -> usize {
    let words: HashSet<String> = keywords(line);
    words.intersection(job).count()
}

fn is_bullet(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ") || t.starts_with("* ") || t.starts_with("• ") || t.starts_with("– ")
}

/// A "Skills: a, b, c" line, split into its label and items.
fn skills_line(line: &str) -> Option<(&str, Vec<&str>)> {
    let (label, rest) = line.split_once(':')?;
    let l = label.trim().to_lowercase();
    if !(l.contains("skill") || l.contains("technolog") || l == "tools" || l == "stack") {
        return None;
    }
    let items: Vec<&str> = rest.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    (items.len() >= 2).then_some((label, items))
}

/// Reorder a resume for a job: within each run of bullets, most relevant first
/// (stable, so equally relevant bullets keep the user's order); on a skills
/// line, the job's skills first. Nothing is added, removed or reworded.
pub fn reorder(resume: &str, job: &str) -> Reordered {
    let jk = keywords(job);
    let lines: Vec<&str> = resume.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut moved: Vec<String> = Vec::new();

    let mut i = 0;
    while i < lines.len() {
        if is_bullet(lines[i]) {
            // Collect this run of bullets, sort it, and note what rose.
            let start = i;
            while i < lines.len() && is_bullet(lines[i]) {
                i += 1;
            }
            let run = &lines[start..i];
            let mut ranked: Vec<(usize, usize, &str)> = run
                .iter()
                .enumerate()
                .map(|(pos, l)| (relevance(l, &jk), pos, *l))
                .collect();
            // Most relevant first; ties keep the original order.
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            for (new_pos, (score, old_pos, line)) in ranked.iter().enumerate() {
                if new_pos < *old_pos && *score > 0 {
                    moved.push(line.trim().trim_start_matches(['-', '*', '•', '–']).trim().to_string());
                }
                out.push((*line).to_string());
            }
            continue;
        }

        if let Some((label, items)) = skills_line(lines[i]) {
            let mut ranked: Vec<(bool, usize, &str)> = items
                .iter()
                .enumerate()
                .map(|(pos, s)| (relevance(s, &jk) > 0, pos, *s))
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            let joined: Vec<&str> = ranked.iter().map(|r| r.2).collect();
            out.push(format!("{label}: {}", joined.join(", ")));
        } else {
            out.push(lines[i].to_string());
        }
        i += 1;
    }

    let resume_words = keywords(resume);
    let mut matched: Vec<String> = jk.intersection(&resume_words).cloned().collect();
    matched.sort();

    Reordered {
        text: out.join("\n"),
        moved,
        matched,
    }
}

/// Claims in generated text that the resume does not back up.
///
/// Two checks. Numbers (with any % or x) that appear in the text but nowhere in
/// the resume; a model writing "improved conversion by 35%" from nothing is the
/// classic fabrication. And capitalised terms from the job ad (product names,
/// "Senior", "GraphQL") that the text uses but the resume never mentions: the
/// model echoing the ad back as if it were the candidate's experience.
///
/// `ignore` lists phrases the text is expected to contain that are not claims:
/// the role being applied for and the company. "I'm applying for the Senior
/// Frontend Engineer role" names the job; "As a Senior engineer" claims it.
/// Stripping the exact title first keeps the second flagged and the first not.
///
/// A third check reads the job's requirement lists ("Must have:",
/// "Requirements:", "Nice to have:") and flags any listed skill the text claims
/// but the resume never mentions. Those are often lowercase ("accessibility"),
/// which the capitalised-term check cannot see, and a cover letter claiming
/// the one must-have the candidate lacks is the costliest invention of all.
pub fn unsupported(text: &str, resume: &str, job: &str, ignore: &[&str]) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    let resume_lower = resume.to_lowercase();

    // Numbers.
    let resume_numbers: HashSet<String> = numbers(resume).into_iter().collect();
    for n in numbers(text) {
        if !resume_numbers.contains(&n) && !flags.contains(&n) {
            flags.push(n);
        }
    }

    // The expected, non-claim phrases come out before any term is checked.
    let mut stripped = format!(" {} ", normalise(text));
    for phrase in ignore {
        let p = normalise(phrase);
        if !p.is_empty() {
            stripped = stripped.replace(&format!(" {p} "), " ");
        }
    }

    // Job-ad terms the resume never uses. Compared as normalised phrases, so a
    // dotted name like "Next.js" matches however it was written.
    let text_norm = stripped;
    let resume_norm = format!(" {} ", normalise(&resume_lower));
    for term in capitalised_terms(job) {
        let phrase = format!(" {} ", normalise(&term));
        if phrase.trim().is_empty() || resume_norm.contains(&phrase) {
            continue;
        }
        if text_norm.contains(&phrase) && !flags.iter().any(|f| f.eq_ignore_ascii_case(&term)) {
            flags.push(term);
        }
    }

    // Required skills the text claims and the resume lacks.
    let resume_keys = keywords(resume);
    let text_keys = keywords(&text_norm);
    for term in requirement_terms(job) {
        if mentioned(&term, &text_keys, &text_norm)
            && !mentioned(&term, &resume_keys, &resume_norm)
            && !flags.iter().any(|f| f.eq_ignore_ascii_case(&term))
        {
            flags.push(term);
        }
    }
    flags
}

/// Phrases that introduce a job's requirements.
const REQUIREMENT_MARKERS: [&str; 6] = [
    "must have",
    "must-have",
    "nice to have",
    "requirements",
    "required",
    "qualifications",
];

/// Generic words that turn up in requirement lists without naming a skill.
const REQUIREMENT_FILLER: [&str; 15] =
    ["frontend", "backend", "engineer", "engineering", "years", "plus", "knowledge", "proficiency",
     "familiarity", "understanding", "ability", "solid", "excellent", "proven", "similar"];

/// The skills a job lists as required or preferred: the items after a marker
/// such as "Must have:", up to the end of that sentence, in the job's own
/// spelling ("Next.js", "WCAG", "CSS") so they read properly when shown.
///
/// Sentences end at ". " rather than at every dot, or "Next.js" would be cut
/// in two. Three-letter skills (CSS, AWS, SQL, Vue) are kept; what keeps the
/// list clean is the stop and filler words, not a length rule.
fn requirement_terms(job: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let sentences = job.replace(". ", "\n").replace("! ", "\n").replace("? ", "\n");
    for sentence in sentences.split('\n') {
        let lower = sentence.to_lowercase();
        let Some(at) = REQUIREMENT_MARKERS
            .iter()
            .find_map(|m| lower.find(m).map(|p| p + m.len()))
        else {
            continue;
        };
        // Offsets from the lowercased copy only line up with the original when
        // lowercasing kept the byte length, which it does for ASCII.
        let rest = if lower.len() == sentence.len() { &sentence[at..] } else { &lower[at..] };
        for raw in rest.split(|c: char| c.is_whitespace() || matches!(c, ',' | '/' | '(' | ')' | ';' | ':')) {
            let term = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '#');
            let low = term.to_lowercase();
            let is_name = term.contains('+') || term.contains('#');
            if (low.len() >= 3 || is_name)
                && !STOP.contains(&low.as_str())
                && !REQUIREMENT_FILLER.contains(&low.as_str())
                && !term.chars().all(|c| c.is_ascii_digit() || c == '+')
                && !out.iter().any(|w| w.eq_ignore_ascii_case(term))
            {
                out.push(term.to_string());
            }
        }
    }
    out
}

/// Whether a term is mentioned in a text: as a phrase for dotted or
/// multi-part names ("Next.js"), else by stemmed keyword, so "testing" in a job
/// is met by "test suites" in a resume.
fn mentioned(term: &str, keys: &HashSet<String>, padded_norm: &str) -> bool {
    let n = normalise(term);
    if n.contains(' ') {
        padded_norm.contains(&format!(" {n} "))
    } else {
        keys.contains(&stem(&n))
    }
}

/// Which of a job's listed requirements the resume covers, and which it does
/// not. Worked out from the text alone, so unlike anything generated it cannot
/// be confidently wrong about the candidate.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub have: Vec<String>,
    pub gaps: Vec<String>,
}

pub fn requirement_coverage(job: &str, resume: &str) -> Coverage {
    let keys = keywords(resume);
    let padded = format!(" {} ", normalise(resume));
    let (mut have, mut gaps) = (Vec::new(), Vec::new());
    for term in requirement_terms(job) {
        if mentioned(&term, &keys, &padded) {
            have.push(term);
        } else {
            gaps.push(term);
        }
    }
    Coverage { have, gaps }
}

/// Interview questions out of the model's text: one per line, list markers
/// and "Q:" stripped, short fragments and duplicates dropped. Not the Learn
/// topic parser, which rejects lines over 120 characters; real interview
/// questions often run longer.
pub fn parse_questions(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let mut line = raw.trim().replace("**", "");
        // Bullets, "1." / "1)" numbering, and a "Q:" label, in any order.
        loop {
            let t = line.trim_start();
            let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
            let next = if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("• ")) {
                Some(r.to_string())
            } else if digits > 0 && (t[digits..].starts_with(". ") || t[digits..].starts_with(") ")) {
                Some(t[digits + 2..].to_string())
            } else if t.len() >= 2 && t[..2].eq_ignore_ascii_case("q:") {
                Some(t[2..].to_string())
            } else {
                None
            };
            match next {
                Some(n) if n != line => line = n,
                _ => break,
            }
        }
        let q = line.trim().trim_matches('"').trim().to_string();
        let lower = q.to_lowercase();
        if q.len() < 15 || q.ends_with(':') || lower.starts_with("here ") || lower.starts_with("sure") {
            continue;
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&q)) {
            out.push(q);
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

/// The coach's suggested first sentence: the text after "Try opening with",
/// unwrapped from quotes and markdown. This is the one part of the feedback
/// that is a claim in the candidate's own voice, so it is the part checked.
pub fn opening_line(feedback: &str) -> Option<String> {
    const MARKER: &str = "try opening with";
    let lower = feedback.to_lowercase();
    let at = lower.find(MARKER)? + MARKER.len();
    let rest = if lower.len() == feedback.len() { &feedback[at..] } else { &lower[at..] };
    let line = rest
        .trim_start_matches(|c: char| c == '*' || c == ':' || c.is_whitespace())
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    let clean = line.replace("**", "");
    let clean = clean.trim().trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}').trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

/// Numeric tokens: digits with an optional decimal part and a trailing % or x.
/// Years are kept too; a date the resume never gave is as invented as a metric.
fn numbers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == ',') {
                i += 1;
            }
            let mut n: String = chars[start..i].iter().collect();
            n = n.trim_end_matches(['.', ',']).replace(',', "");
            if i < chars.len() && (chars[i] == '%' || chars[i] == 'x') {
                n.push(chars[i]);
            }
            out.push(n);
        } else {
            i += 1;
        }
    }
    out
}

/// Words the job ad capitalises mid-sentence, which in a job ad are almost
/// always names: technologies, standards, titles. Sentence-initial words are
/// skipped (capitalised for grammar, not meaning), as are stop words, except
/// at the very start of the ad: that is the job title ("Senior Frontend
/// Engineer"), and a title is exactly what a model copies onto a candidate.
fn capitalised_terms(job: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut sentence_start = false;
    for raw in job.split_whitespace() {
        // Every punctuation mark comes off the ends, dots included, so "(WCAG)."
        // is "WCAG". Dots inside a name survive: "Next.js." is "Next.js".
        let word: String = raw
            .trim_matches(|c: char| !c.is_alphanumeric() && c != '+' && c != '#')
            .to_string();
        // A colon does not end a sentence here: in a job ad it opens a list
        // ("Nice to have: GraphQL, Next.js"), and the first item of that list
        // is a name, not a capitalised sentence opener.
        let ends_sentence = raw.ends_with('.') || raw.ends_with('!') || raw.ends_with('?');
        if !word.is_empty()
            && !sentence_start
            && word.chars().next().is_some_and(|c| c.is_uppercase())
            && word.len() >= 3
            && !STOP.contains(&word.to_lowercase().as_str())
            && !out.contains(&word)
        {
            out.push(word);
        }
        sentence_start = ends_sentence;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESUME: &str = "Rohit R - Frontend Engineer\n\
Experience\n\
Avetta, Frontend Engineer (2022 - present)\n\
- Built supplier search UI in Elm and TypeScript used by 4000 client accounts.\n\
- Led migration of 30 legacy pages to a shared component library (Mercury).\n\
- Cut search page load time from 3.2s to 1.1s by moving filtering server-side.\n\
- Mentored 2 junior engineers; ran weekly UI code review.\n\
Acme Labs, Software Engineer (2019 - 2022)\n\
- React + Redux dashboards for logistics customers.\n\
- Wrote Jest and Cypress test suites; raised coverage from 40% to 78%.\n\
Skills: Elm, CSS, React, TypeScript, Jest, Cypress, Tauri\n\
Education: B.E. Computer Science, Anna University, 2019";

    const JOB: &str = "Senior Frontend Engineer (Remote, APAC). You will own our React + TypeScript \
component library, improve web performance and page load time, mentor engineers, and partner with \
design. Must have: React, TypeScript, testing with Jest or Playwright, accessibility (WCAG). \
Nice to have: GraphQL, Next.js.";

    fn sorted_lines(s: &str) -> Vec<String> {
        let mut v: Vec<String> = s.lines().map(|l| l.trim().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn reordering_never_adds_drops_or_rewords_a_bullet() {
        let r = reorder(RESUME, JOB);
        // Every non-skills line is exactly preserved.
        let keep = |s: &str| -> Vec<String> {
            sorted_lines(s).into_iter().filter(|l| !l.starts_with("Skills:")).collect()
        };
        assert_eq!(keep(&r.text), keep(RESUME));
        // The skills line has the same items, only reordered.
        let skills = |s: &str| -> Vec<String> {
            let line = s.lines().find(|l| l.starts_with("Skills:")).unwrap();
            let mut items: Vec<String> =
                line[7..].split(',').map(|x| x.trim().to_string()).collect();
            items.sort();
            items
        };
        assert_eq!(skills(&r.text), skills(RESUME));
        assert_eq!(r.text.lines().count(), RESUME.lines().count());
    }

    #[test]
    fn relevant_bullets_rise_within_their_own_job_only() {
        let r = reorder(RESUME, JOB);
        let lines: Vec<&str> = r.text.lines().collect();
        let avetta = lines.iter().position(|l| l.starts_with("Avetta")).unwrap();
        let acme = lines.iter().position(|l| l.starts_with("Acme")).unwrap();
        // The component library / load-time bullets outrank "supplier search"
        // for this job, and stay under Avetta.
        let first_avetta_bullet = lines[avetta + 1];
        assert!(
            first_avetta_bullet.contains("component library") || first_avetta_bullet.contains("load time"),
            "{first_avetta_bullet}"
        );
        // No Avetta bullet crossed into Acme's block.
        assert!(lines[acme + 1..].iter().all(|l| !l.contains("Mercury")));
        assert!(!r.moved.is_empty());
    }

    #[test]
    fn skills_the_job_wants_come_first() {
        let r = reorder(RESUME, JOB);
        let line = r.text.lines().find(|l| l.starts_with("Skills:")).unwrap();
        let first_three: Vec<&str> = line[7..].split(',').map(str::trim).take(3).collect();
        for want in ["React", "TypeScript", "Jest"] {
            assert!(first_three.contains(&want), "{line}");
        }
    }

    #[test]
    fn invented_numbers_and_unbacked_job_terms_are_flagged() {
        let letter = "As a Senior engineer I improved conversion by 35% and shipped GraphQL APIs. \
                      I cut load time from 3.2s to 1.1s and mentored 2 engineers using React.";
        let flags = unsupported(letter, RESUME, JOB, &[]);
        assert!(flags.contains(&"35%".to_string()), "{flags:?}");
        assert!(flags.iter().any(|f| f == "GraphQL"), "{flags:?}");
        assert!(flags.iter().any(|f| f == "Senior"), "{flags:?}");
        // Backed by the resume: not flagged.
        for ok in ["3.2", "1.1", "2", "React"] {
            assert!(!flags.iter().any(|f| f == ok), "{ok} wrongly flagged: {flags:?}");
        }
    }

    #[test]
    fn a_faithful_text_raises_no_flags() {
        let summary = "Frontend engineer who led a migration of 30 legacy pages to a shared \
                       component library and cut page load time from 3.2s to 1.1s.";
        assert!(unsupported(summary, RESUME, JOB, &[]).is_empty());
    }

    /// Live, ignored by default: runs the real local model with the production
    /// prompts and prints what the guard flags.
    /// `cargo test --lib tailor::tests::live_guard -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_guard() {
        let model = "llama3.2";
        for (label, prompt) in [
            (
                "summary",
                format!(
                    "Write a 2-sentence professional summary for the top of this resume, aimed at \
                     the Senior Frontend Engineer role at Acme. Use ONLY facts stated in the \
                     resume. Use the candidate's actual current job title from the resume, not \
                     the title in the job ad. Do not invent numbers, skills, or experience.\n\n\
                     RESUME:\n{RESUME}\n\nJOB:\n{JOB}"
                ),
            ),
            (
                "cover",
                format!(
                    "Write a cover letter for the Senior Frontend Engineer role at Acme, from the \
                     candidate whose resume is below. About 200 words, 3 short paragraphs, warm \
                     and direct, no clichés. Connect 2 or 3 specific things from the resume to \
                     what the job asks for. Use ONLY facts from the resume; do not claim any \
                     skill, title, or number the resume does not state. If the job asks for \
                     something the resume lacks, do not mention it. Sign it Rohit.\n\n\
                     RESUME:\n{RESUME}\n\nJOB:\n{JOB}"
                ),
            ),
        ] {
            let started = std::time::Instant::now();
            match crate::assistant::ollama_provider::generate_text(model, &prompt) {
                Ok(text) => {
                    println!("===== {label} ({:.1}s) =====\n{text}", started.elapsed().as_secs_f32());
                    println!("FLAGS: {:?}", unsupported(&text, RESUME, JOB, &["Senior Frontend Engineer", "Acme"]));
                }
                Err(e) => println!("{label} error: {e}"),
            }
        }
    }

    #[test]
    fn naming_the_role_is_not_a_claim_but_wearing_its_title_is() {
        let ignore = ["Senior Frontend Engineer", "Acme"];
        let naming = "I'm excited to apply for the Senior Frontend Engineer role at Acme.";
        assert!(
            !unsupported(naming, RESUME, JOB, &ignore).iter().any(|f| f == "Senior"),
            "naming the role was flagged"
        );
        let claiming = "As a Senior engineer I lead UI work.";
        assert!(
            unsupported(claiming, RESUME, JOB, &ignore).iter().any(|f| f == "Senior"),
            "claiming the title was not flagged"
        );
    }

    #[test]
    fn a_claimed_must_have_the_resume_lacks_is_flagged() {
        // Verbatim from llama3.2's cover letter during the first live run: it
        // claimed accessibility work the resume never mentions. Lowercase in
        // the job ad, so only the requirement-list check can see it.
        let letter = "I can drive performance and accessibility improvements in your library.";
        let flags = unsupported(letter, RESUME, JOB, &[]);
        assert!(flags.iter().any(|f| f == "accessibility"), "{flags:?}");
        // Testing is required by the job and backed by the resume's test suites.
        let backed = "I have written Jest test suites for years.";
        assert!(!unsupported(backed, RESUME, JOB, &[]).iter().any(|f| f.contains("test")));
    }

    #[test]
    fn terms_lose_surrounding_punctuation_but_keep_inner_dots() {
        let terms = capitalised_terms(JOB);
        assert!(terms.iter().any(|t| t == "WCAG"), "{terms:?}");
        assert!(terms.iter().any(|t| t == "Next.js"), "{terms:?}");
        assert!(!terms.iter().any(|t| t.contains(')') || t.contains('(')), "{terms:?}");
        // One claim, one flag: the two checks agree on the spelling.
        let flags = unsupported("We meet WCAG standards.", RESUME, JOB, &[]);
        assert_eq!(flags.iter().filter(|f| f.eq_ignore_ascii_case("wcag")).count(), 1, "{flags:?}");
    }

    #[test]
    fn requirement_lists_are_read_after_their_marker() {
        let terms = requirement_terms(JOB);
        // In the job's own spelling, dotted names intact.
        for want in ["accessibility", "Playwright", "GraphQL", "Next.js", "WCAG"] {
            assert!(terms.iter().any(|t| t == want), "{want} missing from {terms:?}");
        }
        // Words before the marker are not requirements.
        assert!(!terms.iter().any(|t| t == "partner"), "{terms:?}");
    }

    #[test]
    fn coverage_splits_requirements_into_have_and_gaps() {
        let c = requirement_coverage(JOB, RESUME);
        for have in ["React", "TypeScript", "testing", "Jest"] {
            assert!(c.have.iter().any(|t| t == have), "{have} not in have: {c:?}");
        }
        for gap in ["Playwright", "accessibility", "WCAG", "GraphQL", "Next.js"] {
            assert!(c.gaps.iter().any(|t| t == gap), "{gap} not in gaps: {c:?}");
        }
    }

    #[test]
    fn three_letter_skills_count_as_requirements() {
        let job = "Must have: CSS, AWS and SQL. Nice to have: Vue.";
        let terms = requirement_terms(job);
        for want in ["CSS", "AWS", "SQL", "Vue"] {
            assert!(terms.iter().any(|t| t == want), "{want} missing: {terms:?}");
        }
    }

    #[test]
    fn questions_are_read_including_long_ones() {
        // Verbatim shape of llama3.2's output during the first live run.
        let text = "What experience do you have with design systems, and how do you think it would benefit our design-system-driven SaaS?\n\n\
                    1. Can you explain your approach to performance optimization, particularly when it comes to Core Web Vitals, and how you would measure success?\n\
                    - **Q:** How do you mentor junior engineers?\n\
                    Here are the questions:\n\
                    Why?";
        let qs = parse_questions(text, 8);
        assert_eq!(qs.len(), 3, "{qs:?}");
        assert!(qs[1].len() > 120, "long questions must survive");
        assert_eq!(qs[2], "How do you mentor junior engineers?");
    }

    #[test]
    fn the_opening_line_is_extracted_from_the_feedback() {
        let fb = "**Worked**\nGood.\n\n**Try opening with**\n\"I realized that migrating 30 legacy pages was complex.\"";
        assert_eq!(
            opening_line(fb).as_deref(),
            Some("I realized that migrating 30 legacy pages was complex.")
        );
        let inline = "Try opening with: I led the move to a shared component library.";
        assert_eq!(
            opening_line(inline).as_deref(),
            Some("I led the move to a shared component library.")
        );
        assert_eq!(opening_line("No suggestion here."), None);
    }

    #[test]
    fn a_resume_with_no_bullets_comes_back_unchanged() {
        let plain = "Name\nSome paragraph about me.\nSkills: Go";
        assert_eq!(reorder(plain, JOB).text, plain);
    }
}
