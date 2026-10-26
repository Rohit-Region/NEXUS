//! Is a product in stock? The detection half of the stock watcher.
//!
//! Kept apart from the command so the signal, which is the fiddly part, has
//! tests that do not need a network.
//!
//! The authoritative signal on an Amazon product page is the buy box: the
//! page carries an element with `id="add-to-cart-button"` or
//! `id="buy-now-button"` only when the item can actually be bought. The `id="`
//! prefix matters: the bare string `add-to-cart-button` appears once in the
//! page's own JavaScript even when the item is unavailable, so matching it
//! without the id attribute is a false positive that reads every out-of-stock
//! page as in stock. Grepping for "in stock" or "Currently unavailable" fails
//! the other way, because both strings appear in the recommended-product
//! carousels regardless of this item's state.

/// Whether a fetched product page shows a buyable item.
///
/// `None` is not "out of stock": it is "could not tell", for a page that was
/// not a product page at all (a bot wall, a sign-in redirect, an error). The
/// caller keeps the previous state rather than announcing a restock, or a
/// stock-out, that never happened.
pub fn availability(page: &str) -> Option<bool> {
    // A page too small to be a product page is unreadable, not out of stock.
    // A real Amazon product page is hundreds of KB; a bot wall is a few.
    if page.len() < 20_000 {
        return None;
    }

    let buyable =
        page.contains("id=\"add-to-cart-button\"") || page.contains("id=\"buy-now-button\"");
    if buyable {
        return Some(true);
    }

    // No buy box. Only call that "out of stock" when the page also says so in
    // the availability area, so a page whose buy box simply failed to render
    // is read as unknown rather than as a confident stock-out.
    if page.contains("Currently unavailable")
        || page.contains("currently unavailable")
        || page.contains("Out of stock")
    {
        return Some(false);
    }

    None
}

/// A short title for the product, from the page, for the alert to name it.
///
/// Best-effort: the `productTitle` element when it is there, trimmed and
/// length-capped, else nothing and the caller falls back to the URL.
pub fn product_title(page: &str) -> Option<String> {
    let anchor = page.find("id=\"productTitle\"")?;
    let after = &page[anchor..];
    let gt = after.find('>')?;
    let rest = &after[gt + 1..];
    let end = rest.find('<')?;
    let title = rest[..end].trim();
    // Amazon HTML-escapes a few things in the title; undo the common ones so
    // the alert reads naturally.
    let title = title
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"");
    if title.is_empty() {
        None
    } else {
        Some(title.chars().take(120).collect())
    }
}

/// The ASIN from an Amazon product URL.
///
/// Used to match a watched URL against an open browser tab, whose URL for the
/// same product usually carries extra tracking parameters (`?ref=`, `/ref=`)
/// that an exact-string compare would miss. The ASIN is the stable part.
pub fn asin_of(url: &str) -> Option<String> {
    for marker in ["/dp/", "/gp/product/", "/gp/aw/d/", "/product/"] {
        if let Some(idx) = url.find(marker) {
            let rest = &url[idx + marker.len()..];
            let asin: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            if asin.len() >= 8 {
                return Some(asin);
            }
        }
    }
    None
}

/// Read the availability flag the in-tab script returns.
///
/// The script emits `A` (a buy box is present), `U` (the page says
/// unavailable), or `?` (neither, so unknown), then a tab, then the title.
/// Kept here, next to the HTML rule it mirrors, so both readers agree.
pub fn availability_from_flag(flag: &str) -> Option<bool> {
    match flag.trim() {
        "A" => Some(true),
        "U" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(body: &str) -> String {
        // Pad past the size floor so the signal, not the length guard, decides.
        format!("{body}{}", "x".repeat(20_000))
    }

    #[test]
    fn a_buy_button_means_available() {
        assert_eq!(availability(&big("...<input id=\"add-to-cart-button\">...")), Some(true));
        assert_eq!(availability(&big("...id=\"buy-now-button\"...")), Some(true));
    }

    #[test]
    fn the_bare_button_string_in_script_is_not_a_buy_box() {
        // The real trap: `add-to-cart-button` appears once in the page's own
        // JavaScript even when the item is unavailable. Only `id="..."` counts.
        let page = big("...var x='add-to-cart-button';... Currently unavailable ...");
        assert_eq!(availability(&page), Some(false));
    }

    #[test]
    fn no_buy_button_plus_unavailable_text_means_out_of_stock() {
        assert_eq!(availability(&big("...Currently unavailable...")), Some(false));
    }

    #[test]
    fn a_short_page_is_unknown_not_out_of_stock() {
        // A bot wall or error page must never read as a confident answer, or
        // the watcher announces a restock the moment Amazon blocks it.
        assert_eq!(availability("Robot Check. Enter the characters you see below."), None);
    }

    #[test]
    fn a_full_page_with_neither_signal_is_unknown() {
        // Buy box failed to render and no unavailable text: do not guess.
        assert_eq!(availability(&big("a normal looking page with no buybox")), None);
    }

    #[test]
    fn a_carousel_substring_does_not_fake_availability() {
        // "in stock" in a recommendations strip, no buy button: still unknown,
        // because the string is not about this item.
        let page = big("You might also like ... in stock ... related products");
        assert_eq!(availability(&page), None);
    }

    #[test]
    fn the_product_title_is_pulled_and_unescaped() {
        let page = big("<span id=\"productTitle\">XIAOMI 17 (Black &amp; Gold)</span>");
        assert_eq!(product_title(&page).as_deref(), Some("XIAOMI 17 (Black & Gold)"));
    }

    #[test]
    fn asin_is_pulled_from_the_usual_url_shapes() {
        assert_eq!(
            asin_of("https://www.amazon.in/Atomic-Habits-James-Clear/dp/1847941834").as_deref(),
            Some("1847941834")
        );
        assert_eq!(
            asin_of("https://www.amazon.in/dp/B0GMQDDRX8?ref=foo").as_deref(),
            Some("B0GMQDDRX8")
        );
        assert_eq!(
            asin_of("https://www.amazon.in/gp/product/1847941834/ref=x").as_deref(),
            Some("1847941834")
        );
        assert_eq!(asin_of("https://example.com/not-amazon"), None);
    }

    #[test]
    fn the_in_tab_flag_maps_to_availability() {
        assert_eq!(availability_from_flag("A"), Some(true));
        assert_eq!(availability_from_flag("U"), Some(false));
        assert_eq!(availability_from_flag("?"), None);
        assert_eq!(availability_from_flag(""), None);
    }
}
