//! ULIP fund NAVs, for the Money section's insurance-linked holdings.
//!
//! # Source
//!
//! Insurers publish a ULIP fund's NAV daily, but Tata AIA does not offer it
//! in a form a program can read, and these funds are not mutual funds, so
//! AMFI does not carry them. Morningstar India lists every fund of an insurer
//! with its latest NAV and date on one public page per insurer, so that is
//! what this reads. PNB MetLife's funds are priced from the insurer's own NAV
//! chart instead (see `ulip_switch`), which also carries funds too new for
//! Morningstar.
//!
//! The list is an ASP.NET page of 50 rows, and its later pages are reached by
//! posting the page's own form back with the pager link as the event target,
//! which is what a browser does on "2". No cookies are needed.
//!
//! # What it does not do
//!
//! Guess. A fund is priced only when its Morningstar name is known and
//! matches exactly; a fund that is not listed (a new fund in its offer
//! period, say) keeps the NAV the user entered.

use super::http::{send, HttpError, Request};

/// Tata AIA's fund list on Morningstar. Which of its funds are priced from
/// it, and by what name, is in `ulip_switch::TATA_FUNDS`.
pub const TATA_LIST: &str = "https://morningstar.in/insurance/tataaia.aspx";

/// One fund row of an insurer's list.
#[derive(Debug, Clone, PartialEq)]
pub struct Nav {
    pub name: String,
    pub nav: f64,
    /// yyyy-mm-dd.
    pub date: String,
}

/// More than any insurer has (Tata AIA's ~100 funds are two pages); a bound
/// so a page that keeps offering "next" cannot loop forever.
const MAX_PAGES: usize = 6;

/// Every fund an insurer lists, across all pages.
pub fn fetch(url: &str) -> Result<Vec<Nav>, String> {
    let mut page = get(Request::get(url))?;
    let mut navs = rows(&page);
    for number in 2..=MAX_PAGES {
        let Some(target) = pager_target(&page, number) else { break };
        let mut form = hidden_fields(&page);
        form.retain(|(k, _)| k != "__EVENTTARGET" && k != "__EVENTARGUMENT");
        form.push(("__EVENTTARGET".to_string(), target));
        form.push(("__EVENTARGUMENT".to_string(), String::new()));
        let body = form.iter().map(|(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&");
        page = get(Request::post(url, body).with_header("Content-Type", "application/x-www-form-urlencoded"))?;
        navs.extend(rows(&page));
    }
    if navs.is_empty() {
        return Err("Morningstar's fund list had no funds in it; the page may have changed.".to_string());
    }
    Ok(navs)
}

fn get(request: Request) -> Result<String, String> {
    let resp = send(request.with_header("User-Agent", "Mozilla/5.0 (NEXUS)")).map_err(|e: HttpError| match e {
        HttpError::Unreachable { detail } => format!("Could not reach Morningstar: {detail}"),
        other => other.to_string(),
    })?;
    if !resp.ok() {
        return Err(format!("Morningstar returned status {}.", resp.status));
    }
    Ok(resp.body)
}

/// The fund rows of one list page.
fn rows(page: &str) -> Vec<Nav> {
    page.split("<tr class=\"mds-data-table__row\">")
        .skip(1)
        .filter_map(|row| {
            let row = &row[..row.find("</tr>").unwrap_or(row.len())];
            let name = between(row, "title=\"", "\"").map(unescape)?;
            let nav = labelled(row, "Latest NAV")?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0)?;
            let date = iso_date(&labelled(row, "NAV Date")?)?;
            Some(Nav { name, nav, date })
        })
        .collect()
}

/// The text after a cell's `<span class="tiles-label">label</span>`.
fn labelled(row: &str, label: &str) -> Option<String> {
    let marker = format!("<span class=\"tiles-label\">{label}</span>");
    let rest = &row[row.find(&marker)? + marker.len()..];
    let text = rest[..rest.find('<').unwrap_or(rest.len())].trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let to = s[from..].find(end)? + from;
    Some(&s[from..to])
}

/// "Nov 02, 2026" as "2026-11-02".
fn iso_date(raw: &str) -> Option<String> {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let mut parts = raw.split(|c: char| c.is_whitespace() || c == ',').filter(|p| !p.is_empty());
    let word = parts.next()?.to_lowercase();
    let month = MONTHS.iter().position(|m| word.starts_with(m))? + 1;
    let day: u32 = parts.next()?.parse().ok().filter(|d| (1..=31).contains(d))?;
    let year: u32 = parts.next()?.parse().ok().filter(|y| (2000..=2100).contains(y))?;
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// The pager link for page `number`, as the form's event target.
fn pager_target(page: &str, number: usize) -> Option<String> {
    let label = format!(">{number}<");
    page.split("__doPostBack(&#39;").skip(1).find_map(|link| {
        let target = &link[..link.find("&#39;")?];
        let after = &link[..link.find("</a>").map_or(link.len(), |i| i + 4)];
        (target.contains("pagerStocks") && after.replace(char::is_whitespace, "").contains(&label)).then(|| unescape(target))
    })
}

/// The page's hidden inputs, which carry its view state back on a post.
fn hidden_fields(page: &str) -> Vec<(String, String)> {
    page.split("<input type=\"hidden\"")
        .skip(1)
        .filter_map(|input| {
            let tag = &input[..input.find('>')?];
            Some((unescape(between(tag, "name=\"", "\"")?), unescape(between(tag, "value=\"", "\"")?)))
        })
        .collect()
}

fn unescape(s: &str) -> String {
    s.replace("&#39;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Form-encode one value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Names compared without case or runs of spaces.
pub fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, nav: &str, date: &str) -> String {
        format!(
            r#"<tr class="mds-data-table__row">
                <td><a class="identity-cell-value" href="/insurance/f1/x/overview.aspx" title="{name}">{name}</a></td>
                <td><span class="tiles-label">Category</span>
                    Value</td>
                <td><span class="tiles-label">Latest NAV</span>
                    {nav}</td>
                <td><span class="tiles-label">NAV Date</span>
                    {date}
            </tr>"#
        )
    }

    #[test]
    fn rows_are_read_with_their_dates() {
        let page = format!(
            "<table>{}{}</table>",
            row("PNB Metlife Value Fund", "10.7959", "Nov 02, 2026"),
            row("Tata AIG Life - Tax Bonanza &amp; Consumption", "45.1", "Oct 31, 2026"),
        );
        assert_eq!(
            rows(&page),
            vec![
                Nav { name: "PNB Metlife Value Fund".into(), nav: 10.7959, date: "2026-11-02".into() },
                Nav { name: "Tata AIG Life - Tax Bonanza & Consumption".into(), nav: 45.1, date: "2026-10-31".into() },
            ]
        );
    }

    #[test]
    fn a_row_without_a_nav_or_date_is_skipped() {
        let page = format!("{}{}", row("No NAV", "", "Nov 02, 2026"), row("Bad date", "10.1", "soon"));
        assert!(rows(&page).is_empty());
    }

    #[test]
    fn the_pager_and_view_state_are_found() {
        let page = r#"<input type="hidden" name="__VIEWSTATE" id="__VIEWSTATE" value="a+b/c=" />
            <input type="hidden" name="__EVENTTARGET" id="__EVENTTARGET" value="" />
            <a href="javascript:__doPostBack(&#39;ctl00$ContentPlaceHolder1$pagerStocks$ctl02$ctl01&#39;,&#39;&#39;)">
              2</a>
            <a href="javascript:__doPostBack(&#39;ctl00$ContentPlaceHolder1$pagerStocks$ctl03$ctl00&#39;,&#39;&#39;)">Next &gt;</a>"#;
        assert_eq!(pager_target(page, 2).as_deref(), Some("ctl00$ContentPlaceHolder1$pagerStocks$ctl02$ctl01"));
        assert_eq!(pager_target(page, 3), None, "no link for a page that does not exist");
        assert_eq!(hidden_fields(page)[0], ("__VIEWSTATE".to_string(), "a+b/c=".to_string()));
        assert_eq!(encode("a+b/c= $"), "a%2Bb%2Fc%3D%20%24");
    }

    /// Run with `cargo test -- --ignored` to check the Morningstar names
    /// against its page as it is today.
    #[test]
    #[ignore = "network"]
    fn every_named_tata_fund_is_on_morningstar() {
        let navs = fetch(TATA_LIST).unwrap();
        for f in crate::assistant::ulip_switch::TATA_FUNDS {
            let Some(name) = f.morningstar else { continue };
            let found = navs.iter().find(|n| same_name(&n.name, name));
            assert!(found.is_some(), "{}: {name} not in {} funds", f.label, navs.len());
            println!("{}: {:?}", f.label, found.unwrap());
        }
    }

    #[test]
    fn names_match_without_case_or_extra_spaces() {
        assert!(same_name("Tata AIG Life-India  Multicap Opportunities PENSION FUND", "Tata AIG Life-India MULTICAP OPPORTUNITIES PENSION FUND"));
        assert!(!same_name("PNB MetLife Pension Value", "PNB Metlife Value Fund"));
    }
}
