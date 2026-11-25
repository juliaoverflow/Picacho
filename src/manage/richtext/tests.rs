use super::html::{parse, to_html};
use super::*;

fn texts(doc: &Doc) -> Vec<String> {
    doc.blocks.iter().map(|b| b.text()).collect()
}

fn bold() -> Rc<Style> {
    Rc::new(Style { bold: Some(true), ..Style::default() })
}

#[test]
fn paragraphs_lists_and_quotes_are_read_as_blocks() {
    let doc = parse(
        "<p>Hello <b>there</b></p><ul><li>one</li><li>two</li></ul><blockquote><p>quoted</p></blockquote>",
        false,
    );
    assert_eq!(texts(&doc), ["Hello there", "one", "two", "quoted"]);
    let infos = doc.infos(16.);
    assert_eq!(infos[1].marker, Some(Marker::Bullet(1)));
    assert_eq!(infos[2].marker, Some(Marker::Bullet(1)));
    assert_eq!(infos[3].quotes, 1);
    assert_eq!(doc.blocks[0].runs[1].style.bold, Some(true));
    // The two items share their list.
    assert!(Rc::ptr_eq(&doc.blocks[1].path[0], &doc.blocks[2].path[0]));
}

#[test]
fn html_round_trips_without_losing_what_it_does_not_know() {
    let source = "<table class=\"x\" style=\"border:1px solid red\"><tbody><tr><td>A</td><td><span class=\"k\" style=\"letter-spacing:2px\">B</span></td></tr></tbody></table><p style=\"text-align:center\">Mid <font color=\"#ff0000\">red</font></p>";
    let doc = parse(source, false);
    assert_eq!(texts(&doc), ["A", "B", "Mid red"]);
    let out = to_html(&doc);
    assert!(out.starts_with("<table class=\"x\" style=\"border:1px solid red\"><tbody><tr><td>A</td><td>"), "{out}");
    assert!(out.contains("<span class=\"k\" style=\"letter-spacing:2px\">B</span>"), "{out}");
    assert!(out.contains("<p style=\"text-align:center\">Mid <span style=\"color:#ff0000\">red</span></p>"), "{out}");
    // Read again, it is the same.
    assert_eq!(to_html(&parse(&out, false)), out);
}

#[test]
fn scripts_handlers_and_script_links_are_dropped() {
    let doc = parse(
        "<p onclick=\"x()\">Hi<script>alert(1)</script> <a href=\"javascript:alert(1)\">a</a> <a href=\"https://e.com\">b</a></p><iframe src=\"https://e.com\"></iframe><img src=\"file:///etc/passwd\"><img src=\"file:x.png\">",
        false,
    );
    let out = to_html(&doc);
    assert!(!out.contains("onclick") && !out.contains("script") && !out.contains("iframe"), "{out}");
    assert!(out.contains("<a href=\"https://e.com\">b</a>"), "{out}");
    assert!(!out.contains("file:"), "foreign HTML can't name local files: {out}");
    // The draft's own pictures do.
    let own = to_html(&parse("<p><img src=\"file:x.png\" alt=\"X\"></p>", true));
    assert!(own.contains("src=\"file:x.png\""), "{own}");
}

#[test]
fn white_space_collapses_as_a_browser_shows_it() {
    let doc = parse("<div>\n  Two   words \n</div><div>a&nbsp;&nbsp;b<br></div><div><br></div><pre>  kept\n  as is</pre>", false);
    assert_eq!(texts(&doc), ["Two words", "a  b", "", "  kept\n  as is"]);
    let out = to_html(&doc);
    assert!(out.contains("<div>a &nbsp;b</div><div><br></div>"), "{out}");
    assert!(out.contains("<pre>  kept\n  as is</pre>"), "{out}");
}

#[test]
fn typing_splitting_and_joining() {
    let mut doc = Doc::empty();
    let end = doc.insert_text(Pos::new(0, 0), "Hello world", Style::plain());
    assert_eq!(end, Pos::new(0, 11));
    let next = doc.split(Pos::new(0, 5));
    assert_eq!(texts(&doc), ["Hello", " world"]);
    assert_eq!(next, Pos::new(1, 0));
    // The new line is a new element of the same kind.
    assert!(!Rc::ptr_eq(doc.blocks[0].element(), doc.blocks[1].element()));
    let at = doc.delete(Pos::new(0, 5), Pos::new(1, 0));
    assert_eq!(texts(&doc), ["Hello world"]);
    assert_eq!(at, Pos::new(0, 5));
    doc.restyle(Pos::new(0, 0), Pos::new(0, 5), &|s| s.bold = Some(true));
    assert_eq!(to_html(&doc), "<div><b>Hello</b> world</div>");
}

#[test]
fn enter_at_the_end_of_a_heading_writes_an_ordinary_line() {
    let mut doc = parse("<h2>Title</h2>", false);
    let next = doc.split(Pos::new(0, 5));
    doc.insert_text(next, "body", Style::plain());
    assert_eq!(to_html(&doc), "<h2>Title</h2><p>body</p>");
}

#[test]
fn lists_toggle_on_and_off_and_share_one_list() {
    let mut doc = Doc::from_text("one\ntwo\nthree");
    doc.toggle_list(0..=1, ListKind::Bullets);
    assert_eq!(to_html(&doc), "<ul><li>one</li><li>two</li></ul><div>three</div>");
    // The line after joins the list above.
    doc.toggle_list(2..=2, ListKind::Bullets);
    assert_eq!(to_html(&doc), "<ul><li>one</li><li>two</li><li>three</li></ul>");
    doc.toggle_list(0..=2, ListKind::Numbers);
    assert_eq!(to_html(&doc), "<ol><li>one</li><li>two</li><li>three</li></ol>");
    assert_eq!(doc.infos(16.)[2].marker, Some(Marker::Number(3)));
    doc.toggle_list(0..=2, ListKind::Numbers);
    assert_eq!(to_html(&doc), "<div>one</div><div>two</div><div>three</div>");
}

#[test]
fn items_indent_into_the_one_before_and_back() {
    let mut doc = parse("<ul><li>a</li><li>b</li><li>c</li></ul>", false);
    assert!(doc.indent_item(1));
    assert_eq!(to_html(&doc), "<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul>");
    assert!(doc.indent_item(2));
    assert_eq!(to_html(&doc), "<ul><li>a<ul><li>b</li><li>c</li></ul></li></ul>");
    assert_eq!(doc.infos(16.)[2].marker, Some(Marker::Bullet(2)));
    assert!(doc.outdent_item(2));
    assert_eq!(to_html(&doc), "<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul>");
    assert!(doc.outdent_item(2));
    assert_eq!(to_html(&doc), "<ul><li>a<ul><li>b</li></ul></li></ul><div>c</div>");
}

#[test]
fn quotes_wrap_and_unwrap() {
    let mut doc = Doc::from_text("a\nb");
    doc.toggle_quote(0..=1);
    assert_eq!(to_html(&doc), "<blockquote><div>a</div><div>b</div></blockquote>");
    doc.toggle_quote(0..=1);
    assert_eq!(to_html(&doc), "<div>a</div><div>b</div>");
}

#[test]
fn pasted_fragments_join_the_line_they_go_in() {
    let mut doc = Doc::from_text("start end");
    let frag = parse("<p>one</p><p>two</p>", false);
    let at = doc.insert_doc(Pos::new(0, 6), &frag);
    assert_eq!(texts(&doc), ["start one", "twoend"]);
    assert_eq!(at, Pos::new(1, 3));
    let mut doc = Doc::from_text("ab");
    let at = doc.insert_doc(Pos::new(0, 1), &parse("<b>X</b>", false));
    assert_eq!(texts(&doc), ["aXb"]);
    assert_eq!(at, Pos::new(0, 2));
    assert_eq!(doc.blocks[0].runs[1].style.bold, Some(true));
}

#[test]
fn pictures_are_one_character_and_keep_their_attachment() {
    let mut doc = Doc::from_text("ab");
    let image = Image { src: "file:cat.png".into(), alt: "Cat".into(), width: Some(40.), height: None, attrs: vec![] };
    let at = doc.insert_image(Pos::new(0, 1), image, bold());
    assert_eq!(at, Pos::new(0, 1 + OBJECT.len_utf8()));
    assert_eq!(doc.file_images(), ["cat.png"]);
    assert_eq!(to_html(&doc), "<div>a<b><img src=\"file:cat.png\" alt=\"Cat\" width=\"40\"></b>b</div>");
    assert_eq!(doc.to_plain(), "a[Cat]b");
    let sent = html::rewrite_images(&to_html(&doc), &|i| Some(Image { src: "cid:1@x".into(), ..i.clone() }));
    assert!(sent.contains("src=\"cid:1@x\""), "{sent}");
    let gone = html::rewrite_images(&to_html(&doc), &|_| None);
    assert!(gone.contains("aCatb") || gone.contains("a<b>Cat</b>b"), "{gone}");
}

#[test]
fn plain_text_shows_lists_quotes_and_the_signature_line() {
    let doc = parse(
        "<p>Hi</p><p>Text</p><ol><li>one</li><li>two</li></ol><blockquote><div>q</div></blockquote><p>--&nbsp;</p><div>Ana</div>",
        false,
    );
    assert_eq!(doc.to_plain(), "Hi\n\nText\n1. one\n2. two\n> q\n-- \nAna");
    assert_eq!(doc.signature_range(), Some(5..7));
}

#[test]
fn deleting_across_blocks_keeps_the_first_line() {
    let mut doc = parse("<h1>Title</h1><p>one</p><p>two</p>", false);
    doc.delete(Pos::new(0, 2), Pos::new(2, 1));
    assert_eq!(to_html(&doc), "<h1>Tiwo</h1>");
    // A rule goes with a backspace from the line after it.
    let mut doc = parse("<p>a</p><hr><p>b</p>", false);
    doc.delete(Pos::new(1, 0), Pos::new(2, 0));
    assert_eq!(to_html(&doc), "<p>a</p><p>b</p>");
}

#[test]
fn styles_and_body_look_of_other_apps_are_kept() {
    let doc = parse(
        "<html><head><style>.x{color:red}</style></head><body style=\"font-family:Calibri;margin:0\"><p class=\"x\">Hi</p></body></html>",
        false,
    );
    let out = to_html(&doc);
    assert_eq!(out, "<style>.x{color:red}</style><div style=\"font-family:Calibri\"><p class=\"x\">Hi</p></div>");
    assert_eq!(doc.infos(16.)[0].family.as_deref(), Some("Calibri"));
}

#[test]
fn picachos_own_wrapper_is_not_read_as_content() {
    let sent = crate::sync::smtp::html_document_for("<p>Hi</p>");
    assert_eq!(to_html(&parse(&sent, true)), "<p>Hi</p>");
}

#[test]
fn markdown_drafts_open_with_their_formatting_and_pictures() {
    let doc = html::from_markdown("Hi **there**\n\n- a\n- b\n\n![Cat](<file:my cat.png>)", "");
    let out = to_html(&doc);
    assert!(out.contains("<strong>") || out.contains("<b>there</b>"), "{out}");
    assert!(out.contains("<ul><li>a</li><li>b</li></ul>"), "{out}");
    assert!(out.contains("src=\"file:my cat.png\""), "{out}");
    let signed = html::from_markdown("Hello\n\n-- \nAna B", "<p><b>Ana</b> B</p>");
    let out = to_html(&signed);
    assert!(out.contains("<b>Ana</b> B"), "{out}");
    assert!(signed.signature_range().is_some());
}

#[test]
fn deep_markup_does_not_overflow() {
    let html = format!("{}x{}", "<div>".repeat(5000), "</div>".repeat(5000));
    let doc = parse(&html, false);
    assert_eq!(doc.to_plain(), "x");
}

#[test]
fn pictures_take_the_size_they_are_dragged_to() {
    let mut doc = parse("<div>a<img src=\"file:x.png\" alt=\"\" style=\"width:300px;border:0\">b</div>", true);
    assert!(doc.set_image_size(Pos::new(0, 1), 120.4, 80.));
    assert_eq!(to_html(&doc), "<div>a<img src=\"file:x.png\" alt=\"\" width=\"120\" height=\"80\" style=\"border:0\">b</div>");
    assert!(!doc.set_image_size(Pos::new(0, 0), 1., 1.));
}
