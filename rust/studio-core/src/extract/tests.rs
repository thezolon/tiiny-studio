//! Tests for the limits and the fixed reading bugs; every file is built here, tiny.

use super::*;
use std::io::Write;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const ODT: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0""#;

fn zipped(files: &[(&str, &str)]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (n, d) in files {
        z.start_file(*n, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(d.as_bytes()).unwrap();
    }
    z.finish().unwrap().into_inner()
}

fn docx(body: &str) -> Vec<u8> {
    zipped(&[(
        "word/document.xml",
        &format!(r#"<w:document xmlns:w="{W}"><w:body>{body}</w:body></w:document>"#),
    )])
}

fn odt(body: &str) -> Vec<u8> {
    zipped(&[(
        "content.xml",
        &format!(
            r#"<office:document-content {ODT}><office:body><office:text>{body}</office:text></office:body></office:document-content>"#
        ),
    )])
}

fn xlsx(sheet_rows: &str) -> Vec<u8> {
    zipped(&[
        (
            "xl/workbook.xml",
            &format!(
                r#"<workbook xmlns="{S}" xmlns:r="{R}"><sheets><sheet name="S" r:id="rId1"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            &format!(
                r#"<Relationships xmlns="{REL}"><Relationship Id="rId1" Target="/xl/worksheets/sheet1.xml"/></Relationships>"#
            ),
        ),
        (
            "xl/worksheets/sheet1.xml",
            &format!(r#"<worksheet xmlns="{S}"><sheetData>{sheet_rows}</sheetData></worksheet>"#),
        ),
    ])
}

fn text(name: &str, data: &[u8]) -> String {
    extract(name, data).unwrap()[0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn deep_nesting_is_refused_not_a_crash() {
    let n = 5000;
    let body = format!("{}x{}", "<w:r>".repeat(n), "</w:r>".repeat(n));
    let e = extract("a.docx", &docx(&format!("<w:p>{body}</w:p>"))).unwrap_err();
    assert!(e.0.contains("damaged"), "{e}");
    let spans = format!("{}x{}", "<text:span>".repeat(n), "</text:span>".repeat(n));
    assert!(extract("a.odt", &odt(&format!("<text:p>{spans}</text:p>"))).is_err());
}

#[test]
fn counts_taken_from_a_file_are_clamped() {
    let t = text(
        "a.odt",
        &odt(r#"<text:p>a<text:s text:c="100000000000"/>b</text:p>"#),
    );
    assert!(t.len() < 5000, "{}", t.len());
    assert!(t.contains("[… long runs of spaces"), "{t}");
    let li = r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="100000000000"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r></w:p>"#;
    let t = text("a.docx", &docx(li));
    assert!(t.len() < 100, "{}", t.len());
    let (t, md) = render::render(&[
        Block::Li(i64::MAX, false, "x".into()),
        Block::H(i64::MAX, "h".into()),
    ]);
    assert!(t.len() < 100 && md.len() < 100);
    let deep = "<ul><li>".repeat(2000) + "x";
    let t = text("a.html", deep.as_bytes());
    assert!(t.len() < 5000, "{}", t.len());
}

#[test]
fn spreadsheet_columns_and_cells_are_bounded() {
    let rows = r#"<row><c r="A1" t="inlineStr"><is><t>keep</t></is></c><c r="BGQCU1" t="inlineStr"><is><t>far</t></is></c></row>"#;
    assert_eq!(
        text("a.xlsx", &xlsx(rows)),
        "keep\n[… 1 cells beyond column 16384 not shown]"
    );
    let row = r#"<row><c r="A1"><v>1</v></c><c r="XFD1"><v>2</v></c></row>"#;
    let t = text("a.xlsx", &xlsx(&row.repeat(4000)));
    assert!(t.contains("more rows not shown]"), "{t}");
    assert!(
        t.lines().count() <= MAX_CELLS / MAX_COLS + 2,
        "{}",
        t.lines().count()
    );
}

#[test]
fn xlsx_absolute_targets_missing_refs_and_phonetics_are_read() {
    let rows = r#"<row><c t="inlineStr"><is><t>one</t></is></c><c t="inlineStr"><is><r><t>東京</t></r><rPh><t>トウキョウ</t></rPh></is></c></row>"#;
    assert_eq!(text("a.xlsx", &xlsx(rows)), "one\t東京");
}

#[test]
fn zip_limits_count_bytes_really_read() {
    // 70 MiB the zip directory claims are 100 bytes
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    z.start_file("word/document.xml", opts).unwrap();
    z.write_all(&vec![b' '; 70 << 20]).unwrap();
    let mut b = z.finish().unwrap().into_inner();
    let at = b.windows(4).position(|w| w == b"PK\x01\x02").unwrap() + 24;
    b[at..at + 4].copy_from_slice(&100u32.to_le_bytes());
    let e = extract("a.docx", &b).unwrap_err();
    assert!(e.0.contains("too large"), "{e}");
}

#[test]
fn html_scanning_is_linear_and_survives_long_attributes() {
    let long = format!(r#"<p><img alt="{}">after</p>"#, "x".repeat(3_000_000));
    assert_eq!(text("a.html", long.as_bytes()), "after");
    let big = format!("<p>{}</p>", "word ".repeat(300_000));
    assert!(text("a.html", big.as_bytes()).len() > 1_000_000);
    let t0 = Instant::now();
    let many = "<p class=\"x\">Some ordinary paragraph text here.</p>\n".repeat(20_000);
    assert!(text("a.html", many.as_bytes()).contains("ordinary"));
    let bogus = "</>".repeat(50_000) + "<p>end</p>";
    assert_eq!(text("a.html", bogus.as_bytes()), "end");
    assert!(t0.elapsed() < Duration::from_secs(20), "{:?}", t0.elapsed());
}

#[test]
fn html_tokenizer_edge_cases() {
    let t = |h: &str| text("a.html", h.as_bytes());
    assert_eq!(t("<p a='x y' b=\"z\" c=d e>one</p><p/>two"), "one\ntwo");
    assert_eq!(t("<br/>a<p>b</p >"), "a\nb");
}

#[test]
fn html_nested_tables_missing_head_end_and_charrefs() {
    let nested = "<table><tr><td>OUTER ONE</td><td>OUTER TWO<table><tr><td>inner</td></tr></table>AFTER</td></tr><tr><td>ROW2</td></tr></table>";
    let t = text("a.html", nested.as_bytes());
    assert!(
        t.contains("OUTER ONE") && t.contains("OUTER TWO inner AFTER") && t.contains("ROW2"),
        "{t}"
    );
    let nohead = "<html><head><title>T</title><meta charset=utf-8><body><h1>Hello</h1><p>kept</p></body></html>";
    assert_eq!(text("a.html", nohead.as_bytes()), "Hello\nkept");
    assert_eq!(
        text("a.html", b"<head><title>T</title></head><p>x</p>"),
        "x"
    );
    assert_eq!(
        text("a.html", b"<p>don&#146;t &#147;q&#148; &#150; &#0;|</p>"),
        "don\u{2019}t \u{201c}q\u{201d} \u{2013} \u{fffd}|"
    );
    assert_eq!(
        text(
            "a.html",
            b"<table><tr><td><p>Alpha</p><p>Beta</p></td></tr></table>"
        ),
        "Alpha Beta"
    );
}

#[test]
fn rtf_quote_and_dash_words() {
    let rtf =
        br"{\rtf1\ansi It\rquote s a \ldblquote q\rdblquote  \endash  \emdash  \bullet  x.\par}";
    assert_eq!(
        text("a.rtf", rtf),
        "It\u{2019}s a \u{201c}q\u{201d} \u{2013} \u{2014} \u{2022} x."
    );
}

#[test]
fn latin1_text_of_even_length_is_not_utf16() {
    assert_eq!(
        text("a.txt", b"Caf\xe9 menu: cr\xe8me br\xfbl\xe9e!"),
        "Caf\u{e9} menu: cr\u{e8}me br\u{fb}l\u{e9}e!"
    );
    let s = "h\u{e9}llo w\u{f6}rld";
    let le: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    assert_eq!(text("a.txt", &le), s);
    let be: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
    assert_eq!(text("a.txt", &be), s);
}

#[test]
fn ods_keeps_empty_repeated_cells() {
    let ods = format!(
        r#"<office:document-content {ODT}><office:body><office:spreadsheet><table:table table:name="S"><table:table-row><table:table-cell><text:p>a</text:p></table:table-cell><table:table-cell table:number-columns-repeated="3"/><table:table-cell><text:p>b</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    assert_eq!(
        text("a.ods", &zipped(&[("content.xml", &ods)])),
        "a\t\t\t\tb"
    );
}

#[test]
fn docx_text_box_and_content_control_cells() {
    let mc = "http://schemas.openxmlformats.org/markup-compatibility/2006";
    let body = format!(
        r#"<w:p xmlns:mc="{mc}"><w:r><w:t>before </w:t></w:r><mc:AlternateContent><mc:Choice><w:p><w:r><w:t>BOX</w:t></w:r></w:p></mc:Choice><mc:Fallback><w:p><w:r><w:t>BOX</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:sdt><w:sdtContent><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:sdtContent></w:sdt><w:tc><w:p><w:r><w:t>C</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#
    );
    let t = text("a.docx", &docx(&body));
    assert_eq!(t.matches("BOX").count(), 1, "{t}");
    assert!(t.ends_with("A\tB\tC"), "{t}");
}

#[test]
fn csv_oddities_fall_back_instead_of_failing() {
    assert_eq!(text("a.csv", b"a,b\rc,d\r"), "a\tb\nc\td");
    let t = text("a.csv", b"a,b\n\"stray,quote\nmore text\n");
    assert!(t.contains("more text"), "{t}");
}

#[test]
fn epub_hrefs_are_urls() {
    assert_eq!(epub::unquote("Chapter%201.xhtml"), "Chapter 1.xhtml");
}

#[tokio::test]
async fn isolated_extract_returns_the_result() {
    let r = extract_isolated("a.txt".into(), b"hello".to_vec())
        .await
        .unwrap();
    assert_eq!(r[0]["text"], "hello");
}

#[test]
fn rows_cut_by_the_limit_say_so() {
    let csv = "a,b\n".repeat(MAX_ROWS + 7);
    let t = text("a.csv", csv.as_bytes());
    assert!(
        t.ends_with("[… more rows not shown]"),
        "{}",
        &t[t.len() - 40..]
    );
    let csv = "a,b\n".repeat(MAX_ROWS);
    assert!(!text("a.csv", csv.as_bytes()).contains("not shown"));
    let rows = r#"<row><c t="inlineStr"><is><t>x</t></is></c></row>"#.repeat(MAX_ROWS + 3);
    assert!(text("a.xlsx", &xlsx(&rows)).ends_with("[… 3 more rows not shown]"));
}

#[test]
fn ods_cut_repeats_say_so() {
    let ods = format!(
        r#"<office:document-content {ODT}><office:body><office:spreadsheet><table:table table:name="S"><table:table-row><table:table-cell table:number-columns-repeated="9999"><text:p>a</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#
    );
    let t = text("a.ods", &zipped(&[("content.xml", &ods)]));
    assert!(t.contains("[… some repeated cells were shortened]"), "{t}");
}

#[test]
fn slides_follow_the_decks_own_order() {
    const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
    let slide = |t: &str| {
        format!(
            r#"<p:sld xmlns:p="{P}" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
        )
    };
    let pres = format!(
        r#"<p:presentation xmlns:p="{P}" xmlns:r="{R}"><p:sldIdLst><p:sldId id="1" r:id="rId2"/><p:sldId id="2" r:id="rId1"/></p:sldIdLst></p:presentation>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="{REL}"><Relationship Id="rId1" Target="slides/slide1.xml"/><Relationship Id="rId2" Target="slides/slide2.xml"/></Relationships>"#
    );
    let z = zipped(&[
        ("ppt/presentation.xml", &pres),
        ("ppt/_rels/presentation.xml.rels", &rels),
        ("ppt/slides/slide1.xml", &slide("FIRSTFILE")),
        ("ppt/slides/slide2.xml", &slide("SECONDFILE")),
    ]);
    let parts = extract("a.pptx", &z).unwrap();
    assert_eq!(parts[0]["text"], "SECONDFILE");
    assert_eq!(parts[0]["label"], "Slide 1");
    assert_eq!(parts[1]["text"], "FIRSTFILE");
}
