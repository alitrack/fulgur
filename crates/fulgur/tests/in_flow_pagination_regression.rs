//! Regression tests for in-flow tall content pagination (fulgur-sbw2).
//!
//! A plain `<div style="height:300vh">` must occupy three pages — the
//! viewport height is one page, `300vh` is three viewports tall, and the
//! body has `margin:0`, so the in-flow content drives a 3-page render.
//! Pre-regression this passed (see WPT `fixedpos-003-print` history); a
//! recent pagination_layout change collapsed it back to 1 page.

use fulgur::Engine;

fn page_count(pdf: &[u8]) -> usize {
    let prefix = b"/Type /Page";
    let mut count = 0usize;
    let mut i = 0;
    while i + prefix.len() < pdf.len() {
        if &pdf[i..i + prefix.len()] == prefix {
            let next = pdf[i + prefix.len()];
            if !next.is_ascii_alphanumeric() {
                count += 1;
            }
            i += prefix.len();
        } else {
            i += 1;
        }
    }
    count
}

#[test]
fn in_flow_300vh_paginates_to_three_pages() {
    let html = r#"<!DOCTYPE html>
<body style="margin:0">
<div style="height:300vh">x</div>
</body>"#;
    let pdf = Engine::builder().build().render_html(html).expect("render");
    let pages = page_count(&pdf);
    assert_eq!(pages, 3, "expected 3 pages, got {pages}");
}
