//! Spending: what was paid, to whom, and on which day.
//!
//! # Where the data comes from
//!
//! Bank SMS, forwarded by the user's phone into a Google Sheet they own
//! (`bank_sms`), plus anything entered by hand. Google Pay has no API for its
//! history, but every payment it makes produces a bank SMS, so the SMS is the
//! one record that is always there.
//!
//! # Reading a message, and when not to trust the reading
//!
//! Banks word their alerts freely and change them without notice. A misread
//! amount is worse than a missing one: it silently corrupts every total. So a
//! message is booked automatically only when it matches a shape seen in a real
//! message from that bank (`known_shape`). Anything else is still read, but
//! goes to the user as a suggestion to confirm with one tap. Messages that are
//! not payments at all (balance alerts, dues, collect requests) are set aside
//! with the reason, never counted.
//!
//! # What counts as spending
//!
//! Only `Kind::Spend`. A credit card bill paid from the bank account is a
//! transfer: the purchases were already counted when made on the card, and
//! counting the bill again would double them. Investments and money sent to
//! yourself are transfers for the same reason. Income and refunds are shown
//! beside spending, never netted into it.

use std::collections::HashMap;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Spend,
    Income,
    Refund,
    Transfer,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Spend => "spend",
            Kind::Income => "income",
            Kind::Refund => "refund",
            Kind::Transfer => "transfer",
        }
    }

    pub fn parse(raw: &str) -> Option<Kind> {
        match raw {
            "spend" => Some(Kind::Spend),
            "income" => Some(Kind::Income),
            "refund" => Some(Kind::Refund),
            "transfer" => Some(Kind::Transfer),
            _ => None,
        }
    }
}

/// The categories offered everywhere a category is picked. Fixed so totals
/// group cleanly; "Uncategorised" is where an unknown payee waits.
pub const CATEGORIES: &[&str] = &[
    "Food",
    "Groceries",
    "Shopping",
    "Bills",
    "Transport",
    "Fuel",
    "Travel",
    "Tour",
    "Sports",
    "Health",
    "Entertainment",
    "Rent",
    "Insurance",
    "Education",
    "Family",
    "People",
    "Investments",
    "Other",
    "Uncategorised",
];

pub const UNCATEGORISED: &str = "Uncategorised";

pub fn valid_category(raw: &str) -> bool {
    CATEGORIES.contains(&raw)
}

/// One payment read from a bank message.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub amount_paise: Option<i64>,
    pub debit: Option<bool>,
    /// YYYY-MM-DD, as the bank dated it.
    pub date: Option<String>,
    pub payee: Option<String>,
    pub account: String,
    pub channel: String,
    pub card: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A shape seen in a real message, fully read: book it.
    Booked(Reading),
    /// Read as far as possible; the user confirms or corrects it.
    Review { reading: Reading, reason: String },
    /// Not a payment.
    Ignored { reason: String },
}

/// Phrases that mark a bank message as something other than a completed
/// payment: a future debit, a request, a due date, a statement.
const NOT_A_PAYMENT: &[(&str, &str)] = &[
    ("will be debited", "A future debit, not a payment yet"),
    ("has requested", "A payment request, not a payment"),
    ("requested money", "A payment request, not a payment"),
    ("collect request", "A payment request, not a payment"),
    ("is due", "A due-date reminder"),
    ("due date", "A due-date reminder"),
    ("due by", "A due-date reminder"),
    ("minimum amount due", "A card statement"),
    ("min amt due", "A card statement"),
    ("total amt due", "A card statement"),
    ("total amount due", "A card statement"),
    ("statement for", "A card statement"),
    ("mandate", "An autopay setup, not a payment"),
    ("e-mandate", "An autopay setup, not a payment"),
    ("failed", "A failed transaction"),
    ("declined", "A declined transaction"),
];

/// `received_on` (YYYY-MM-DD) supplies the year when a bank leaves it out
/// ("On 22-10").
pub fn read_sms(sender: &str, text: &str, received_on: &str) -> Outcome {
    let chars: Vec<char> = text.chars().collect();
    let lower: String = text.to_lowercase();

    for (phrase, reason) in NOT_A_PAYMENT {
        if lower.contains(phrase) {
            return Outcome::Ignored { reason: (*reason).to_string() };
        }
    }

    let amount = transaction_amount(&chars);
    let Some(amount_paise) = amount else {
        return Outcome::Ignored { reason: "No amount in the message".to_string() };
    };

    let card = has_word(&lower, "card");
    let debit = direction(&lower, card);
    let date_match = find_date(&chars).or_else(|| find_short_date(&chars, received_on));
    let bank = bank_of(sender);
    let digits = account_digits(&lower);
    let account = match (&digits, card) {
        (Some(d), true) => format!("{bank} card XX{d}"),
        (Some(d), false) => format!("{bank} XX{d}"),
        (None, true) => format!("{bank} card"),
        (None, false) => bank.to_string(),
    };
    let payee = find_payee(&chars, &lower, date_match.as_ref().map(|d| d.end))
        // A credit often names no sender ("credited with Rs 500 on ..."); the
        // money still arrived, and the user can name it once.
        .or_else(|| (debit == Some(false)).then(|| "Money received".to_string()));

    let reading = Reading {
        amount_paise: Some(amount_paise),
        debit,
        date: date_match.map(|d| d.iso),
        payee,
        account,
        channel: channel_of(&lower, card),
        card,
    };

    let complete = reading.debit.is_some() && reading.date.is_some() && reading.payee.is_some();
    if complete && known_shape(sender, &lower) {
        Outcome::Booked(reading)
    } else {
        let reason = if complete {
            "A message format NEXUS has not seen from this bank yet".to_string()
        } else {
            let mut missing = Vec::new();
            if reading.debit.is_none() {
                missing.push("whether money went out or came in");
            }
            if reading.date.is_none() {
                missing.push("the date");
            }
            if reading.payee.is_none() {
                missing.push("who was paid");
            }
            format!("Could not read {}", missing.join(", "))
        };
        Outcome::Review { reading, reason }
    }
}

/// Shapes taken from real messages. Adding a bank's format is adding a line
/// here, with a test built from the real message.
fn known_shape(sender: &str, lower: &str) -> bool {
    let icici = sender.to_uppercase().contains("ICICI");
    // "ICICI Bank Acct XX225 debited for Rs 1.00 on 24-Oct-26; NAME credited. UPI:..."
    let icici_upi = icici
        && lower.contains("debited for rs")
        && lower.contains(" credited")
        && lower.contains("upi:");
    // "ICICI Bank Acc XX225 debited Rs. 2,295.99 on 23-Oct-26 PAYEE.Avl Bal Rs. ..."
    let icici_debit_with_balance = icici && lower.contains("debited rs") && lower.contains("avl bal");
    // "Txn Rs.925.00\nOn HDFC Bank Card 5131\nAt PAYEE \nby UPI ...\nOn 22-10\nNot You?..."
    let hdfc = sender.to_uppercase().contains("HDFC");
    let hdfc_card_txn = hdfc
        && lower.trim_start().starts_with("txn rs")
        && lower.contains("on hdfc bank card")
        && lower.contains("\nat ");
    // Money into the ICICI account: salary, transfers in, refunds. Any
    // wording, as long as it says credited and never debited, so a UPI debit
    // ("... X credited") is not mistaken for one. Booked as income or refund;
    // a transfer from the user's own account is marked Moved once and
    // remembered.
    let icici_credit = icici && has_word(lower, "credited") && !lower.contains("debited");
    icici_upi || icici_debit_with_balance || hdfc_card_txn || icici_credit
}

fn bank_of(sender: &str) -> &'static str {
    let s = sender.to_uppercase();
    if s.contains("ICICI") {
        "ICICI"
    } else if s.contains("HDFC") {
        "HDFC"
    } else {
        "Bank"
    }
}

fn channel_of(lower: &str, card: bool) -> String {
    let c = if lower.contains("upi") {
        "UPI"
    } else if card {
        "Card"
    } else if lower.contains("neft") || lower.contains("imps") || lower.contains("rtgs") {
        "Bank transfer"
    } else if lower.contains("infobil") || lower.contains("billpay") || lower.contains("bbps") {
        "Bill payment"
    } else if lower.contains("atm") {
        "ATM"
    } else {
        ""
    };
    c.to_string()
}

/// `word` with no letter on either side, so "card" is not found in "discard"
/// and "sent" not in "present".
fn has_word(lower: &str, word: &str) -> bool {
    lower.match_indices(word).any(|(i, _)| {
        let before = lower[..i].chars().next_back();
        let after = lower[i + word.len()..].chars().next();
        !before.is_some_and(|c| c.is_alphabetic()) && !after.is_some_and(|c| c.is_alphabetic())
    })
}

/// Debit or credit. A UPI debit names both ("debited ... X credited"), so the
/// debit words win: the account holder's side is the one being reported.
fn direction(lower: &str, card: bool) -> Option<bool> {
    const DEBIT: &[&str] = &["debited", "spent", "sent", "paid", "withdrawn", "purchase"];
    const CREDIT: &[&str] = &["credited", "received", "deposited"];
    if DEBIT.iter().any(|w| has_word(lower, w)) {
        Some(true)
    } else if CREDIT.iter().any(|w| has_word(lower, w)) {
        Some(false)
    } else if card && has_word(lower, "txn") {
        // HDFC card alerts say only "Txn Rs.925.00 On HDFC Bank Card": a
        // card transaction with no credit word is a purchase.
        Some(true)
    } else {
        None
    }
}

/// The last digits of the account or card: "XX225", "x1234", "*1234",
/// "ending 1234".
fn account_digits(lower: &str) -> Option<String> {
    let chars: Vec<char> = lower.chars().collect();
    let digits_at = |mut i: usize| -> String {
        let mut out = String::new();
        while i < chars.len() && chars[i].is_ascii_digit() && out.len() < 6 {
            out.push(chars[i]);
            i += 1;
        }
        out
    };
    for i in 0..chars.len() {
        let prev_ok = i == 0 || !chars[i - 1].is_alphanumeric();
        // "xx225" / "xxxx1234"
        if prev_ok && chars[i] == 'x' {
            let mut j = i;
            while j < chars.len() && (chars[j] == 'x' || chars[j] == '*') {
                j += 1;
            }
            let d = digits_at(j);
            if d.len() >= 3 {
                return Some(d);
            }
        }
        // "*1234"
        if chars[i] == '*' {
            let d = digits_at(i + 1);
            if d.len() >= 3 {
                return Some(d);
            }
        }
    }
    // "Card 5131"
    if let Some(i) = lower.find("card ") {
        let d: String = lower[i + 5..].chars().take_while(|c| c.is_ascii_digit()).collect();
        if d.len() == 4 {
            return Some(d);
        }
    }
    if let Some(i) = lower.find("ending") {
        let rest: Vec<char> = lower[i + 6..].chars().collect();
        let d: String = rest
            .iter()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if d.len() >= 3 {
            return Some(d);
        }
    }
    None
}

/// Every "Rs 1,234.50" / "INR 500" / "₹99" in the text, with where it starts.
fn amounts(chars: &[char]) -> Vec<(usize, i64)> {
    let mut out = Vec::new();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let mut i = 0;
    while i < lower.len() {
        let marker_len = if lower[i] == '₹' {
            1
        } else if lower[i..].starts_with(&['r', 's']) && (i == 0 || !lower[i - 1].is_alphabetic()) {
            2
        } else if lower[i..].starts_with(&['i', 'n', 'r']) && (i == 0 || !lower[i - 1].is_alphabetic()) {
            3
        } else {
            0
        };
        if marker_len == 0 {
            i += 1;
            continue;
        }
        let mut j = i + marker_len;
        while j < lower.len() && (lower[j] == '.' || lower[j] == ' ' || lower[j] == ':') {
            j += 1;
        }
        let start = j;
        let mut rupees: i64 = 0;
        let mut seen = false;
        while j < lower.len() && (lower[j].is_ascii_digit() || lower[j] == ',') {
            if let Some(d) = lower[j].to_digit(10) {
                rupees = rupees.saturating_mul(10).saturating_add(d as i64);
                seen = true;
            }
            j += 1;
        }
        if !seen || j == start {
            i += marker_len;
            continue;
        }
        let mut paise = 0;
        if j + 1 < lower.len() && lower[j] == '.' && lower[j + 1].is_ascii_digit() {
            let tens = lower[j + 1].to_digit(10).unwrap_or(0) as i64;
            let ones = lower
                .get(j + 2)
                .and_then(|c| c.to_digit(10))
                .map(|d| d as i64);
            paise = tens * 10 + ones.unwrap_or(0);
        }
        out.push((i, rupees * 100 + paise));
        i = j;
    }
    out
}

/// The balance a bank states after a transaction ("Avl Bal Rs. 34,709.16",
/// "Available Balance is Rs. ..."): the last amount right after the word
/// "bal". A card's available limit is not a balance and is never taken.
pub fn stated_balance(text: &str) -> Option<i64> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    if lower.len() != chars.len() {
        return None;
    }
    amounts(&chars)
        .into_iter()
        .filter(|(at, _)| {
            let before: String = lower[at.saturating_sub(24)..*at].iter().collect();
            before.contains("bal") && !before.contains("lmt") && !before.contains("limit")
        })
        .map(|(_, paise)| paise)
        .last()
}

/// The account a message is about, named the way expenses name it, for
/// matching a stated balance to that account's payments.
pub fn account_of(sender: &str, text: &str) -> String {
    let lower = text.to_lowercase();
    let bank = bank_of(sender);
    let bank = if bank == "Bank" { bank_of(text) } else { bank };
    match account_digits(&lower) {
        Some(d) if has_word(&lower, "card") => format!("{bank} card XX{d}"),
        Some(d) => format!("{bank} XX{d}"),
        None => bank.to_string(),
    }
}

/// A balance carried forward: the last known one, plus money in and less
/// money out on that account since. Spending and transfers (a card bill paid
/// from the account) leave it; income and refunds arrive in it.
pub fn carried_balance(known: i64, since: &[(Kind, i64)]) -> i64 {
    since.iter().fold(known, |b, (kind, paise)| match kind {
        Kind::Income | Kind::Refund => b + paise,
        Kind::Spend | Kind::Transfer => b - paise,
    })
}

/// The payment's amount: the first one that is not a balance or limit.
fn transaction_amount(chars: &[char]) -> Option<i64> {
    let lower: String = chars.iter().collect::<String>().to_lowercase();
    let lower_chars: Vec<char> = lower.chars().collect();
    amounts(chars)
        .into_iter()
        .find(|(at, paise)| {
            let from = at.saturating_sub(24);
            let before: String = lower_chars[from..*at].iter().collect();
            *paise > 0 && !before.contains("bal") && !before.contains("lmt") && !before.contains("limit")
        })
        .map(|(_, paise)| paise)
}

struct DateMatch {
    iso: String,
    end: usize,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// The first date in the text: 24-Oct-26, 24-Oct-2026, 24/10/26, 24-10-2026
/// or 2026-10-24.
fn find_date(chars: &[char]) -> Option<DateMatch> {
    for i in 0..chars.len() {
        if !chars[i].is_ascii_digit() || (i > 0 && chars[i - 1].is_alphanumeric()) {
            continue;
        }
        if let Some(m) = date_at(chars, i) {
            return Some(m);
        }
    }
    None
}

/// "On 22-10" or "on 22/10": a day and month with no year, which takes the
/// year of `received_on`, or the year before if that would put it in the
/// future (a 31 Dec message read on 1 Jan).
fn find_short_date(chars: &[char], received_on: &str) -> Option<DateMatch> {
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();
    let (ry, rm, rd) = {
        let p: Vec<&str> = received_on.split('-').collect();
        (p.first()?.parse::<i64>().ok()?, p.get(1)?.parse::<u32>().ok()?, p.get(2)?.parse::<u32>().ok()?)
    };
    for i in 0..lower.len().saturating_sub(3) {
        if !(lower[i] == 'o' && lower[i + 1] == 'n' && lower[i + 2] == ' ')
            || (i > 0 && lower[i - 1].is_alphanumeric())
        {
            continue;
        }
        let mut j = i + 3;
        let take = |j: &mut usize| {
            let mut s = String::new();
            while *j < lower.len() && lower[*j].is_ascii_digit() && s.len() < 3 {
                s.push(lower[*j]);
                *j += 1;
            }
            s
        };
        let day = take(&mut j);
        let Some(&sep) = lower.get(j) else { continue };
        if day.is_empty() || day.len() > 2 || (sep != '-' && sep != '/') {
            continue;
        }
        j += 1;
        let month = take(&mut j);
        // A third part means a full date, which `find_date` handles.
        let followed = lower.get(j).copied();
        if month.is_empty() || month.len() > 2 || followed.is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '/') {
            continue;
        }
        let (d, m) = (day.parse::<u32>().ok()?, month.parse::<u32>().ok()?);
        if !(1..=12).contains(&m) {
            continue;
        }
        let y = if (m, d) > (rm, rd) { ry - 1 } else { ry };
        if d == 0 || d > days_in_month(y, m) {
            continue;
        }
        return Some(DateMatch { iso: format!("{y:04}-{m:02}-{d:02}"), end: j });
    }
    None
}

fn date_at(chars: &[char], start: usize) -> Option<DateMatch> {
    let take_digits = |mut i: usize, max: usize| -> (String, usize) {
        let mut s = String::new();
        while i < chars.len() && chars[i].is_ascii_digit() && s.len() < max {
            s.push(chars[i]);
            i += 1;
        }
        (s, i)
    };
    let (first, i) = take_digits(start, 4);
    let sep = *chars.get(i)?;
    if sep != '-' && sep != '/' {
        return None;
    }
    // Middle part: digits or a month name.
    let (month, j) = if chars.get(i + 1)?.is_ascii_digit() {
        let (m, j) = take_digits(i + 1, 2);
        (m.parse::<u32>().ok()?, j)
    } else {
        let name: String = chars.get(i + 1..i + 4)?.iter().collect::<String>().to_lowercase();
        let m = MONTHS.iter().position(|n| *n == name)? as u32 + 1;
        (m, i + 4)
    };
    if *chars.get(j)? != sep {
        return None;
    }
    let (last, end) = take_digits(j + 1, 4);
    if chars.get(end).is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let (y, d) = if first.len() == 4 {
        (first.parse::<i64>().ok()?, last.parse::<u32>().ok()?)
    } else {
        let y = match last.len() {
            2 => 2000 + last.parse::<i64>().ok()?,
            4 => last.parse::<i64>().ok()?,
            _ => return None,
        };
        (y, first.parse::<u32>().ok()?)
    };
    if !(1..=12).contains(&month) || d == 0 || d > days_in_month(y, month) {
        return None;
    }
    Some(DateMatch { iso: format!("{y:04}-{month:02}-{d:02}"), end })
}

/// Who was paid (or who paid). Tried in order of how specific the cue is.
fn find_payee(chars: &[char], lower: &str, date_end: Option<usize>) -> Option<String> {
    let mut text: String = chars.iter().collect();
    // Positions found in `lower` are used to slice `text`. Lowercasing keeps
    // byte offsets for everything a bank sends, but not for every character
    // in Unicode; if they ever differ, read the payee from `lower` instead of
    // slicing at the wrong place.
    if text.len() != lower.len() {
        text = lower.to_string();
    }

    // ICICI UPI: "...on 24-Oct-26; NAME credited."
    if let Some(semi) = lower.find("; ") {
        if let Some(rel) = lower[semi..].find(" credited") {
            return clean_payee(&text[semi + 2..semi + rel]);
        }
    }
    // A debit with the balance after it: "...on 23-Oct-26 PAYEE.Avl Bal..."
    if let (Some(end), Some(bal)) = (date_end, lower.find("avl bal")) {
        let end_byte = text.char_indices().nth(end).map(|(b, _)| b).unwrap_or(text.len());
        if bal > end_byte {
            if let Some(p) = clean_payee(&text[end_byte..bal]) {
                return Some(p);
            }
        }
    }
    for cue in [" at ", "\nat ", " to ", "\nto ", " from ", "\nfrom ", " by ", " info:", " info "] {
        if let Some(i) = lower.find(cue) {
            let start = i + cue.len();
            let rest = &lower[start..];
            let stop = [" on ", "\non ", ". ", ".\n", "\n", " ref", " upi", " avl", "(", ";"]
                .iter()
                .filter_map(|s| rest.find(s))
                .min()
                .unwrap_or(rest.len());
            if let Some(p) = clean_payee(&text[start..start + stop]) {
                return Some(p);
            }
        }
    }
    None
}

fn clean_payee(raw: &str) -> Option<String> {
    let cleaned = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c == '.' || c == ',' || c == ';' || c == ':' || c == '-' || c.is_whitespace())
        .to_string();
    let lower = cleaned.to_lowercase();
    let useless = ["", "your", "you", "a/c", "your a/c", "account", "the"];
    if useless.contains(&lower.as_str()) || cleaned.chars().count() > 60 {
        return None;
    }
    // Pure numbers (a reference, an account) are not a payee.
    if cleaned.chars().all(|c| c.is_ascii_digit() || c == ' ') {
        return None;
    }
    Some(cleaned)
}

/// A stable key for a payee, so "SWIGGY", "Swiggy " and "swiggy." share one
/// learned category.
pub fn payee_key(payee: &str) -> String {
    payee
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(48)
        .collect()
}

/// Spending, income, refund or transfer, from the message and payee alone.
pub fn guess_kind(debit: bool, card: bool, payee: &str, text: &str) -> Kind {
    let p = payee.to_lowercase();
    let t = text.to_lowercase();
    if debit {
        let card_bill = t.contains("credit card payment")
            || t.contains("card bill")
            || t.contains("cc payment")
            || (!card && (p.contains("credit card") || (p.contains("card") && p.contains("hdfc"))));
        if card_bill {
            Kind::Transfer
        } else {
            Kind::Spend
        }
    } else if t.contains("refund") || t.contains("reversal") || t.contains("reversed") || t.contains("cashback") {
        Kind::Refund
    } else if card {
        // Money arriving on a credit card is the bill being paid.
        Kind::Transfer
    } else {
        Kind::Income
    }
}

/// A first guess at the category from well-known payee names. Short names
/// must match a whole word ("ola" is not "Nikola"); longer ones a substring.
pub fn guess_category(payee: &str) -> &'static str {
    const RULES: &[(&str, &[&str])] = &[
        ("Food", &["swiggy", "zomato", "dominos", "mcdonald", "kfc", "starbucks", "eatsure", "restaurant", "cafe", "bakery", "pizza", "burger"]),
        ("Groceries", &["blinkit", "zepto", "bigbasket", "instamart", "dmart", "jiomart", "grocer", "supermarket", "freshtohome", "milkbasket"]),
        ("Shopping", &["amazon", "flipkart", "myntra", "ajio", "meesho", "nykaa", "croma", "ikea", "lenskart"]),
        ("Bills", &["airtel", "jio", "vodafone", "bescom", "electricity", "broadband", "fibernet", "infobil", "billdesk", "bbps", "recharge", "tataplay", "dth"]),
        ("Transport", &["uber", "ola", "rapido", "metro", "fastag", "namma yatri", "blusmart"]),
        ("Fuel", &["petrol", "hpcl", "bpcl", "iocl", "indianoil", "indian oil", "fuel"]),
        ("Travel", &["irctc", "makemytrip", "goibibo", "indigo", "airindia", "cleartrip", "redbus", "ixigo", "oyo", "airbnb"]),
        ("Tour", &["tour", "trek", "thrillophilia", "tripoto"]),
        ("Sports", &["decathlon", "playo", "hudle", "cultfit", "cult.fit", "gym", "turf", "badminton", "cricket", "football", "sports"]),
        ("Health", &["apollo", "pharmeasy", "netmeds", "hospital", "clinic", "pharmacy", "medplus", "practo", "1mg"]),
        ("Entertainment", &["netflix", "spotify", "hotstar", "bookmyshow", "primevideo", "sonyliv", "youtube", "pvr", "inox"]),
        ("Rent", &["rent", "nobroker"]),
        ("Education", &["udemy", "coursera", "school", "college", "tuition"]),
        // Tata AIA here is the user's pension ULIP: a premium into it is
        // money saved, not spent.
        ("Investments", &["tata aia", "tataaia", "zerodha", "groww", "kuvera", "upstox", "angel one", "mutual fund", "icicisec", "nps", "ppf", "sip"]),
        ("Insurance", &["insurance", "policybazaar", "lic"]),
    ];
    let lower = payee.to_lowercase();
    for (category, words) in RULES {
        for w in *words {
            let hit = if w.len() <= 4 { has_word(&lower, w) } else { lower.contains(w) };
            if hit {
                return category;
            }
        }
    }
    UNCATEGORISED
}

/// Investments are money kept, not money spent.
pub fn kind_for_category(category: &str, kind: Kind) -> Kind {
    if kind == Kind::Spend && category == "Investments" {
        Kind::Transfer
    } else {
        kind
    }
}

// -- Calendar arithmetic ------------------------------------------------------

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Monday = 0 ... Sunday = 6.
pub fn weekday(y: i64, m: u32, d: u32) -> u32 {
    (days_from_civil(y, m, d) + 3).rem_euclid(7) as u32
}

/// "2026-10" as (2026, 10).
pub fn parse_month(raw: &str) -> Option<(i64, u32)> {
    let (y, m) = raw.split_once('-')?;
    let y: i64 = y.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    ((1..=12).contains(&m) && (2000..=2100).contains(&y)).then_some((y, m))
}

pub fn shift_month((y, m): (i64, u32), by: i32) -> (i64, u32) {
    let index = y * 12 + m as i64 - 1 + by as i64;
    (index.div_euclid(12), (index.rem_euclid(12) + 1) as u32)
}

pub fn valid_date(raw: &str) -> bool {
    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    match (parts[0].parse::<i64>(), parts[1].parse::<u32>(), parts[2].parse::<u32>()) {
        (Ok(y), Ok(m), Ok(d)) => (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m),
        _ => false,
    }
}

// -- The month view -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Expense {
    pub id: i64,
    pub source: String,
    pub occurred_on: String,
    pub occurred_at: Option<i64>,
    pub amount_paise: i64,
    pub kind: Kind,
    pub payee: String,
    pub category: String,
    pub account: String,
    pub channel: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayTotal {
    pub date: String,
    pub spend_paise: i64,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotal {
    pub category: String,
    pub spend_paise: i64,
    pub count: u32,
    pub budget_paise: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayeeTotal {
    pub payee: String,
    pub category: String,
    pub spend_paise: i64,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTotal {
    pub account: String,
    pub spend_paise: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthView {
    pub month: String,
    pub today: String,
    pub days_in_month: u32,
    /// Weekday of the 1st, Monday = 0, for laying out the calendar.
    pub first_weekday: u32,
    pub days: Vec<DayTotal>,
    pub categories: Vec<CategoryTotal>,
    pub payees: Vec<PayeeTotal>,
    pub accounts: Vec<AccountTotal>,
    /// Spending per weekday, Monday first.
    pub weekdays: Vec<i64>,
    pub spend_paise: i64,
    pub income_paise: i64,
    pub refund_paise: i64,
    pub transfer_paise: i64,
    /// Last month's spending over the same number of days, for "vs last month".
    pub previous_paise: i64,
    pub transactions: Vec<Expense>,
    pub all_categories: Vec<String>,
    pub savings: Savings,
    /// The last six months, oldest first, ending with this one.
    pub history: Vec<MonthSavings>,
    /// The usual monthly income the user set, used when a month has no
    /// recorded income.
    pub usual_income_paise: Option<i64>,
    /// Bank account balances, filled in by the caller (not month-specific).
    pub balances: Vec<Balance>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub account: String,
    /// The last known balance carried forward to now.
    pub balance_paise: i64,
    /// When the last known balance was stated, unix seconds.
    pub as_of: i64,
    /// "sms" or "manual".
    pub source: String,
    /// Payments on the account since then, already applied.
    pub adjusted: u32,
}

/// What was kept in a month: money in (income plus refunds) less spending.
/// Investments and card bills are transfers, never subtracted, so money
/// invested counts as saved.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Savings {
    /// Income used: recorded, else the usual income, else none.
    pub income_paise: Option<i64>,
    /// True when `income_paise` is the usual income, not what was recorded.
    pub estimated: bool,
    pub saved_paise: Option<i64>,
    /// Saved as a share of income, in percent.
    pub rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonthSavings {
    pub month: String,
    pub spend_paise: i64,
    pub savings: Savings,
}

/// Salary is taken as arrived once a single credit reaches half the usual
/// income. Until then the usual income stands in, plus any smaller credits,
/// so ₹500 from a friend early in the month does not become the whole
/// month's income.
const SALARY_SHARE: f64 = 0.5;

pub fn savings(income: i64, largest_income: i64, refund: i64, spend: i64, usual: Option<i64>) -> Savings {
    let usual = usual.filter(|u| *u > 0);
    let (income, estimated) = match usual {
        Some(u) if (largest_income as f64) < u as f64 * SALARY_SHARE => (Some(u + income), true),
        Some(_) => (Some(income), false),
        None if income > 0 => (Some(income), false),
        None => (None, false),
    };
    match income {
        None => Savings { income_paise: None, estimated: false, saved_paise: None, rate: None },
        Some(i) => {
            let saved = i + refund - spend;
            Savings {
                income_paise: Some(i),
                estimated,
                saved_paise: Some(saved),
                rate: Some((saved as f64 / i as f64 * 1000.0).round() / 10.0),
            }
        }
    }
}

/// Everything the Spending tab shows for one month, from its expenses.
/// Pure, so every total is tested without a database.
pub fn summarize(
    (y, m): (i64, u32),
    today: &str,
    mut rows: Vec<Expense>,
    budgets: &HashMap<String, i64>,
    previous_paise: i64,
    usual_income_paise: Option<i64>,
    history: Vec<MonthSavings>,
) -> MonthView {
    let dim = days_in_month(y, m);
    let mut days: Vec<DayTotal> = (1..=dim)
        .map(|d| DayTotal { date: format!("{y:04}-{m:02}-{d:02}"), spend_paise: 0, count: 0 })
        .collect();
    let mut categories: HashMap<String, (i64, u32)> = HashMap::new();
    let mut payees: HashMap<String, (String, String, i64, u32)> = HashMap::new();
    let mut accounts: HashMap<String, i64> = HashMap::new();
    let mut weekdays = vec![0i64; 7];
    let (mut spend, mut income, mut refund, mut transfer) = (0, 0, 0, 0);
    let mut largest_income = 0;

    for e in &rows {
        match e.kind {
            Kind::Income => {
                income += e.amount_paise;
                largest_income = largest_income.max(e.amount_paise);
            }
            Kind::Refund => refund += e.amount_paise,
            Kind::Transfer => transfer += e.amount_paise,
            Kind::Spend => {
                spend += e.amount_paise;
                let day: u32 = e.occurred_on.get(8..10).and_then(|d| d.parse().ok()).unwrap_or(0);
                if (1..=dim).contains(&day) {
                    let slot = &mut days[(day - 1) as usize];
                    slot.spend_paise += e.amount_paise;
                    slot.count += 1;
                    weekdays[weekday(y, m, day) as usize] += e.amount_paise;
                }
                let c = categories.entry(e.category.clone()).or_default();
                c.0 += e.amount_paise;
                c.1 += 1;
                let p = payees
                    .entry(payee_key(&e.payee))
                    .or_insert_with(|| (e.payee.clone(), e.category.clone(), 0, 0));
                p.2 += e.amount_paise;
                p.3 += 1;
                *accounts.entry(e.account.clone()).or_default() += e.amount_paise;
            }
        }
    }

    let mut categories: Vec<CategoryTotal> = categories
        .into_iter()
        .map(|(category, (spend_paise, count))| CategoryTotal {
            budget_paise: budgets.get(&category).copied(),
            category,
            spend_paise,
            count,
        })
        .collect();
    // A budgeted category with nothing spent yet still shows, so its limit is
    // visible from the first day of the month.
    for (category, limit) in budgets {
        if !categories.iter().any(|c| &c.category == category) {
            categories.push(CategoryTotal {
                category: category.clone(),
                spend_paise: 0,
                count: 0,
                budget_paise: Some(*limit),
            });
        }
    }
    categories.sort_by(|a, b| b.spend_paise.cmp(&a.spend_paise).then(a.category.cmp(&b.category)));

    let mut payees: Vec<PayeeTotal> = payees
        .into_values()
        .map(|(payee, category, spend_paise, count)| PayeeTotal { payee, category, spend_paise, count })
        .collect();
    payees.sort_by(|a, b| b.spend_paise.cmp(&a.spend_paise).then(a.payee.cmp(&b.payee)));
    payees.truncate(10);

    let mut accounts: Vec<AccountTotal> = accounts
        .into_iter()
        .map(|(account, spend_paise)| AccountTotal { account, spend_paise })
        .collect();
    accounts.sort_by(|a, b| b.spend_paise.cmp(&a.spend_paise));

    rows.sort_by(|a, b| {
        b.occurred_on
            .cmp(&a.occurred_on)
            .then(b.occurred_at.cmp(&a.occurred_at))
            .then(b.id.cmp(&a.id))
    });

    MonthView {
        month: format!("{y:04}-{m:02}"),
        today: today.to_string(),
        days_in_month: dim,
        first_weekday: weekday(y, m, 1),
        days,
        categories,
        payees,
        accounts,
        weekdays,
        spend_paise: spend,
        income_paise: income,
        refund_paise: refund,
        transfer_paise: transfer,
        previous_paise,
        transactions: rows,
        all_categories: CATEGORIES.iter().map(|c| c.to_string()).collect(),
        savings: savings(income, largest_income, refund, spend, usual_income_paise),
        history,
        usual_income_paise,
        balances: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENDER: &str = "AX-ICICIT-S";
    /// Real messages, with long reference numbers replaced.
    const ICICI_UPI: &str = "ICICI Bank Acct XX225 debited for Rs 1.00 on 24-Oct-26; godisalwaysgrea credited. UPI:123456789012. Call 18002662 for dispute. SMS BLOCK 225 to 9215676766.";
    const ICICI_BILL: &str = "ICICI Bank Acc XX225 debited Rs. 2,295.99 on 23-Oct-26 InfoBIL*INFT*FJR0.Avl Bal Rs. 34,709.16.To dispute call 18002662 or SMS BLOCK 225 to 9215676766";

    fn booked(outcome: Outcome) -> Reading {
        match outcome {
            Outcome::Booked(r) => r,
            other => panic!("expected booked, got {other:?}"),
        }
    }


    #[test]
    fn the_real_icici_upi_message_is_booked() {
        let r = booked(read_sms(SENDER, ICICI_UPI, "2026-10-24"));
        assert_eq!(r.amount_paise, Some(100));
        assert_eq!(r.debit, Some(true), "debited wins over the payee's 'credited'");
        assert_eq!(r.date.as_deref(), Some("2026-10-24"));
        assert_eq!(r.payee.as_deref(), Some("godisalwaysgrea"));
        assert_eq!(r.account, "ICICI XX225");
        assert_eq!(r.channel, "UPI");
        assert!(!r.card);
    }

    #[test]
    fn the_real_icici_bill_message_is_booked_and_the_balance_is_not_the_amount() {
        let r = booked(read_sms(SENDER, ICICI_BILL, "2026-10-24"));
        assert_eq!(r.amount_paise, Some(229_599), "Rs. 2,295.99, not the Avl Bal 34,709.16");
        assert_eq!(r.date.as_deref(), Some("2026-10-23"));
        assert_eq!(r.payee.as_deref(), Some("InfoBIL*INFT*FJR0"));
        assert_eq!(r.debit, Some(true));
    }

    #[test]
    fn the_real_hdfc_card_message_is_booked_with_the_year_filled_in() {
        let text = "Txn Rs.925.00\nOn HDFC Bank Card 5131\nAt *u766@pty \nby UPI 131673777866\nOn 22-10\nNot You?\nCall 18002586161/SMS BLOCK CC 5131 to 7308080808";
        let r = booked(read_sms("VM-HDFCBK-S", text, "2026-10-24"));
        assert_eq!(r.amount_paise, Some(92_500));
        assert_eq!(r.debit, Some(true), "a card Txn is a purchase");
        assert_eq!(r.date.as_deref(), Some("2026-10-22"));
        assert_eq!(r.payee.as_deref(), Some("*u766@pty"));
        assert_eq!(r.account, "HDFC card XX5131");
        assert_eq!(r.channel, "UPI");
        assert_eq!(guess_kind(true, true, "*u766@pty", text), Kind::Spend);
    }

    #[test]
    fn a_date_without_a_year_never_lands_in_the_future() {
        let short = |s: &str, on: &str| find_short_date(&s.chars().collect::<Vec<_>>(), on).map(|d| d.iso);
        assert_eq!(short("On 31-12\nNot You?", "2027-01-01"), Some("2026-12-31".into()));
        assert_eq!(short("On 22/10", "2026-10-24"), Some("2026-10-22".into()));
        assert_eq!(short("On 22-10-26", "2026-10-24"), None, "a full date is find_date's job");
        assert_eq!(short("Call 18002586161", "2026-10-24"), None);
    }

    #[test]
    fn the_stated_balance_is_read_and_never_the_payment_or_a_card_limit() {
        assert_eq!(stated_balance(ICICI_BILL), Some(3_470_916));
        assert_eq!(stated_balance(ICICI_UPI), None, "a UPI debit states no balance");
        assert_eq!(stated_balance("credited:Rs. 500.00 on 24-Oct-26. Available Balance is Rs. 34,709.16."), Some(3_470_916));
        assert_eq!(stated_balance("Rs 450 spent on card. Avl Lmt: Rs 50,000"), None);
        assert_eq!(account_of(SENDER, ICICI_BILL), "ICICI XX225");
        assert_eq!(account_of("PASTED-ICICI", ICICI_BILL), "ICICI XX225");
    }

    #[test]
    fn a_balance_is_carried_forward_through_later_payments() {
        let since = [(Kind::Spend, 92_500), (Kind::Income, 10_000_000), (Kind::Transfer, 2_000_000), (Kind::Refund, 1_000)];
        assert_eq!(carried_balance(2_400_000, &since), 2_400_000 - 92_500 + 10_000_000 - 2_000_000 + 1_000);
        assert_eq!(carried_balance(2_400_000, &[]), 2_400_000);
    }

    #[test]
    fn icici_credits_are_booked_as_money_in() {
        // The exact wording of ICICI credits has not been seen yet, so these
        // two common shapes stand in; each must book with the amount, not the
        // balance, and never be read as a debit.
        for (text, amount) in [
            ("Dear Customer, Acct XX225 is credited with Rs 1,00,000.00 on 31-Oct-26 from AVETTA SOFTWARE. UPI:123456789012-ICICI Bank.", 10_000_000),
            ("ICICI Bank Account XX225 credited:Rs. 500.00 on 24-Oct-26. Info NEFT-HDFC0001-FRIEND. Available Balance is Rs. 34,709.16.", 50_000),
        ] {
            let r = booked(read_sms(SENDER, text, "2026-10-31"));
            assert_eq!(r.amount_paise, Some(amount), "{text}");
            assert_eq!(r.debit, Some(false), "{text}");
            assert_eq!(r.account, "ICICI XX225");
            assert!(r.payee.is_some());
        }
        let refund = "Acct XX225 is credited with Rs 120.00 on 24-Oct-26 as refund from AMAZON";
        assert_eq!(guess_kind(false, false, "AMAZON", refund), Kind::Refund);
        // A UPI debit names the payee as "credited" and must stay a debit.
        assert_eq!(booked(read_sms(SENDER, ICICI_UPI, "2026-10-24")).debit, Some(true));
    }

    #[test]
    fn an_unseen_format_is_read_but_waits_for_the_user() {
        // Invented, so it must not be booked on NEXUS's word alone.
        let text = "Rs.450.00 spent on HDFC Bank Card x4321 at SWIGGY on 2026-10-24. Not you? Call 18002586161";
        match read_sms("VM-HDFCBK", text, "2026-10-24") {
            Outcome::Review { reading, reason } => {
                assert_eq!(reading.amount_paise, Some(45_000));
                assert_eq!(reading.payee.as_deref(), Some("SWIGGY"));
                assert_eq!(reading.date.as_deref(), Some("2026-10-24"));
                assert_eq!(reading.account, "HDFC card XX4321");
                assert!(reading.card);
                assert!(reason.contains("not seen"), "{reason}");
            }
            other => panic!("expected review, got {other:?}"),
        }
    }

    #[test]
    fn what_is_missing_is_named() {
        let text = "Your a/c is debited with Rs 120.";
        match read_sms("JD-ICICIT", text, "2026-10-24") {
            Outcome::Review { reason, reading } => {
                assert_eq!(reading.amount_paise, Some(12_000));
                assert!(reason.contains("the date"), "{reason}");
                assert!(reason.contains("who was paid"), "{reason}");
            }
            other => panic!("expected review, got {other:?}"),
        }
    }

    #[test]
    fn non_payments_are_set_aside_with_a_reason() {
        for text in [
            "Rs 5,000 will be debited from your ICICI Bank Acct XX225 on 01-Nov-26 towards SIP",
            "HDFC Bank Credit Card XX4321: Total Amt Due Rs 12,345.00, Min Amt Due Rs 620.00. Pay by 05-Nov-26",
            "ABC has requested money from you on Google Pay. Rs 200",
            "Your ICICI Bank Acct XX225 balance is updated",
        ] {
            assert!(matches!(read_sms(SENDER, text, "2026-10-24"), Outcome::Ignored { .. }), "{text}");
        }
    }

    #[test]
    fn dates_in_every_bank_style() {
        let iso = |s: &str| find_date(&s.chars().collect::<Vec<_>>()).map(|d| d.iso);
        assert_eq!(iso("on 24-Oct-26;"), Some("2026-10-24".into()));
        assert_eq!(iso("on 24-OCT-2026 at"), Some("2026-10-24".into()));
        assert_eq!(iso("On 24/10/26\nRef"), Some("2026-10-24".into()));
        assert_eq!(iso("on 2026-10-24."), Some("2026-10-24".into()));
        assert_eq!(iso("on 31-02-26"), None, "no 31st of February");
        assert_eq!(iso("UPI:123456789012"), None);
    }

    #[test]
    fn amounts_in_every_style_and_never_from_a_word() {
        let all = |s: &str| amounts(&s.chars().collect::<Vec<_>>()).into_iter().map(|(_, p)| p).collect::<Vec<_>>();
        assert_eq!(all("Rs.1,23,456.5 then INR 20 and ₹99.99"), vec![12_345_650, 2_000, 9_999]);
        assert_eq!(all("hrs 50 and others 10"), Vec::<i64>::new(), "'rs' inside a word is not rupees");
    }

    #[test]
    fn a_card_bill_paid_from_the_bank_is_a_transfer_not_spending() {
        assert_eq!(guess_kind(true, false, "HDFC Credit Card", "debited"), Kind::Transfer);
        assert_eq!(guess_kind(true, false, "x", "towards credit card payment"), Kind::Transfer);
        assert_eq!(guess_kind(true, true, "SWIGGY", "spent on card"), Kind::Spend);
        assert_eq!(guess_kind(false, true, "x", "payment received on card"), Kind::Transfer);
        assert_eq!(guess_kind(false, false, "x", "refund of Rs 20"), Kind::Refund);
        assert_eq!(guess_kind(false, false, "ACME PAYROLL", "credited"), Kind::Income);
    }

    #[test]
    fn categories_are_guessed_without_false_matches() {
        assert_eq!(guess_category("SWIGGY LIMITED"), "Food");
        assert_eq!(guess_category("InfoBIL*INFT*FJR0"), "Bills");
        assert_eq!(guess_category("Ola Cabs"), "Transport");
        assert_eq!(guess_category("Nikola Tesla"), UNCATEGORISED, "'ola' inside a word");
        assert_eq!(guess_category("godisalwaysgrea"), UNCATEGORISED);
        assert_eq!(kind_for_category("Investments", Kind::Spend), Kind::Transfer);
        assert_eq!(guess_category("DECATHLON SPORTS INDIA"), "Sports");
        assert_eq!(guess_category("Playo turf booking"), "Sports");
        assert_eq!(guess_category("Coorg Trek Co"), "Tour");
        assert_eq!(guess_category("Tourmaline Gems"), UNCATEGORISED, "'tour' inside a word");
        assert!(valid_category("Tour") && valid_category("Sports"));
        assert_eq!(guess_category("TATA AIA LIFE INSURANCE CO"), "Investments", "checked before Insurance");
        assert_eq!(guess_category("HDFC ERGO INSURANCE"), "Insurance");
    }

    #[test]
    fn payee_keys_ignore_case_and_punctuation() {
        assert_eq!(payee_key("Swiggy."), payee_key(" SWIGGY"));
    }

    #[test]
    fn calendar_arithmetic() {
        assert_eq!(weekday(2026, 10, 1), 3, "1 Oct 2026 is a Thursday");
        assert_eq!(weekday(2024, 2, 29), 3, "29 Feb 2024 was a Thursday");
        assert_eq!(days_in_month(2028, 2), 29);
        assert_eq!(shift_month((2026, 1), -1), (2025, 12));
        assert_eq!(shift_month((2026, 12), 1), (2027, 1));
        assert!(valid_date("2026-10-24") && !valid_date("2026-13-01") && !valid_date("24-10-2026"));
    }

    fn expense(id: i64, on: &str, paise: i64, kind: Kind, payee: &str, category: &str) -> Expense {
        Expense {
            id,
            source: "sms".into(),
            occurred_on: on.into(),
            occurred_at: None,
            amount_paise: paise,
            kind,
            payee: payee.into(),
            category: category.into(),
            account: "ICICI XX225".into(),
            channel: "UPI".into(),
            note: String::new(),
        }
    }

    #[test]
    fn the_month_counts_only_spending_and_lays_out_each_day() {
        let rows = vec![
            expense(1, "2026-10-24", 100, Kind::Spend, "godisalwaysgrea", "People"),
            expense(2, "2026-10-23", 229_599, Kind::Spend, "InfoBIL", "Bills"),
            expense(3, "2026-10-23", 50_000, Kind::Spend, "Swiggy", "Food"),
            expense(4, "2026-10-24", 5_000_000, Kind::Transfer, "HDFC Credit Card", "Other"),
            expense(5, "2026-10-01", 10_000_000, Kind::Income, "Salary", "Other"),
            expense(6, "2026-10-02", 2_000, Kind::Refund, "Amazon", "Shopping"),
        ];
        let budgets = HashMap::from([("Food".to_string(), 1_000_000), ("Travel".to_string(), 500_000)]);
        let v = summarize((2026, 10), "2026-10-24", rows, &budgets, 200_000, Some(10_000_000), Vec::new());

        assert_eq!(v.spend_paise, 100 + 229_599 + 50_000, "transfer, income and refund are not spending");
        assert_eq!(v.transfer_paise, 5_000_000);
        assert_eq!(v.income_paise, 10_000_000);
        assert_eq!(v.refund_paise, 2_000);
        assert_eq!(v.days.len(), 31);
        assert_eq!(v.first_weekday, 3);
        assert_eq!(v.days[22], DayTotal { date: "2026-10-23".into(), spend_paise: 279_599, count: 2 });
        assert_eq!(v.days[23].spend_paise, 100, "the card bill on the 24th is not in the day's spending");
        assert_eq!(v.categories[0].category, "Bills");
        let food = v.categories.iter().find(|c| c.category == "Food").unwrap();
        assert_eq!(food.budget_paise, Some(1_000_000));
        let travel = v.categories.iter().find(|c| c.category == "Travel").unwrap();
        assert_eq!((travel.spend_paise, travel.budget_paise), (0, Some(500_000)), "a budget shows before any spending");
        assert_eq!(v.payees[0].payee, "InfoBIL");
        assert_eq!(v.weekdays[4], 279_599, "23 Oct 2026 is a Friday");
        assert_eq!(v.transactions[0].occurred_on, "2026-10-24", "newest first");
        assert_eq!(v.previous_paise, 200_000);
        // Income 1,00,000 + refund 20 - spend 2,796.99; the 50,000 card bill is not subtracted.
        assert_eq!(v.savings.saved_paise, Some(10_000_000 + 2_000 - 279_699));
        assert!(!v.savings.estimated, "recorded income wins over the usual income");
    }

    #[test]
    fn savings_use_recorded_income_else_the_usual_one_else_none() {
        let usual = Some(10_000_000); // ₹1,00,000
        // Salary arrived (one credit of ₹1,02,000): the recorded income is used.
        let paid = savings(10_250_000, 10_200_000, 0, 6_000_000, usual);
        assert_eq!((paid.saved_paise, paid.estimated), (Some(4_250_000), false));
        // Only ₹500 from a friend so far: usual income plus that ₹500.
        let early = savings(50_000, 50_000, 0, 3_000_000, usual);
        assert_eq!((early.income_paise, early.estimated), (Some(10_050_000), true));
        // Nothing recorded: the usual income.
        let est = savings(0, 0, 10_000, 3_000_000, Some(8_000_000));
        assert_eq!((est.saved_paise, est.estimated), (Some(5_010_000), true));
        let over = savings(1_000_000, 1_000_000, 0, 1_500_000, None);
        assert_eq!((over.saved_paise, over.rate), (Some(-500_000), Some(-50.0)), "overspending is negative");
        assert_eq!(savings(0, 0, 0, 100, None).saved_paise, None, "no income known, no guess");
    }
}
