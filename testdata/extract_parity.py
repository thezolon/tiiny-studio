"""Parity check for the Rust document reader: every file in testdata/docs/ (plus a few built-in edge cases)
goes through Python's extract.extract and studio-core's extract (the `extract` example), and the two results
must be identical: same parts, or the same ExtractError message. Where Python raises some other exception
(server.py answers "Couldn't read this file (...)"), Rust only has to fail too.
Runs make_docs.py first when testdata/docs/ is missing. Usage: python extract_parity.py   (exit 1 on any diff)"""
import io
import json
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(ROOT))
import extract  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")
DOCS = HERE / "docs"
RUST = ROOT / "rust"


def build_example():
    """Build the Rust example once and return its executable (falls back to `cargo run` per file)."""
    out = subprocess.run(["cargo", "build", "-q", "-p", "studio-core", "--example", "extract", "--message-format", "json"],
                         cwd=RUST, capture_output=True, text=True)
    if out.returncode:
        sys.exit("cargo build failed:\n" + out.stderr[-3000:])
    for line in out.stdout.splitlines():
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("executable") and msg.get("target", {}).get("name") == "extract":
            return [msg["executable"]]
    return ["cargo", "run", "-q", "-p", "studio-core", "--example", "extract", "--"]


def zipped(files):
    b = io.BytesIO()
    with zipfile.ZipFile(b, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            z.writestr(name, data)
    return b.getvalue()


# Edge cases beyond make_docs.py: errors, damaged files, and messy markup.
EXTRA = {
    "fake.doc": b"\xd0\xcf\x11\xe0 not really a Word file",
    "fake.xls": b"\xd0\xcf\x11\xe0",
    "fake.ppt": b"\xd0\xcf\x11\xe0",
    "corrupt.docx": b"this is not a zip file at all",
    "empty.docx": b"",
    "other.zip": zipped({"readme.txt": "hello"}),
    "no-document.docx": zipped({"word/other.xml": "<a/>"}),
    "bad-xml.docx": zipped({"word/document.xml": "<w:document><unclosed>"}),
    "no-body.docx": zipped({"word/document.xml": '<d xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>'}),
    "no-container.epub": zipped({"mimetype": "application/epub+zip"}),
    "empty-rootfile.epub": zipped({"META-INF/container.xml": "<container><rootfiles><rootfile/></rootfiles></container>"}),
    "binary.bin": bytes(range(256)) * 4,
    "empty.txt": b"",
    "latin1.txt": "caf\xe9 \x80 na\xefve".encode("latin-1"),
    "utf16.txt": "﻿Hello utf16\n\nsecond paragraph".encode("utf-16"),
    "bom.txt": b"\xef\xbb\xbfBOM text\r\n\r\n\r\nafter blank lines\r\n",
    "paragraphs.txt": b"one\n \t \ntwo\nline\n\n\n\nthree\n  \n  four",
    "special.md": b"  # Title  \n\n- item *one*\n",
    "bad.json": b'{"a": 1,}',
    "nested.json": json.dumps({"k": [1, 2.5, {"x": None, "y": [True, False]}, 1e22, 1e-7], "é": "ü\n ", "e": {}, "l": []}).encode(),
    "dup.json": b'{"a": 1, "b": 2, "a": 3, "big": 123456789012345678901234567890, "f": 0.1, "n": -0, "nan": NaN}',
    "semi.csv": b"Name;Qty;Note\nApples;3;a b\nPears;5;\"x;y\"\n",
    "quoted.csv": b'a,b,c\n"x ""q"" y",2,"multi\nline"\n1,2\n\n3,4,5\n',
    "tabs.tsv": b"a\tb\n1\t2\n",
    "pipes.csv": b"a|b|c\n1|2|3\n4|5|6\n",
    "single.csv": b"just\none\ncolumn\n",
    "messy.html": (b"<!DOCTYPE html><html><head><title>T</title><style>p{color:red}</style></head><body>"
                   b"<h1>Head &amp; <em>tail</em></h1><p>Para one<br>line two &copy; &#x41;&#66; &bogus; &lt;b&gt;</p>"
                   b"<div>div text</div><ul><li>a<ul><li>nested</li></ul></li><li>b</li></ul>"
                   b"<ol><li>1</li><li>2</li></ol><table><tr><th>H1<th>H2<tr><td>x<td>y | z</table>"
                   b"<pre>  keep\n    this  </pre><!-- comment --><script>if (a < b) { document.write('<p>x</p>') }</script>"
                   b"<p>after <b>bold</b> <a href='x>y'>link</a></p><![CDATA[ cdata ]]><p>end</body></html>"),
    "broken.html": b"<html><body><p>open <b>tag <a href=\"u\">x</a> <p>second &amp <div<span>z</span>",
    "unterminated.html": b"<html><body><p>text <b",
    # unfinished constructs at the end of the input (html.parser drops them since its 2025 fix)
    "eof-attr.html": b"<p>a <b x='1",
    "eof-endtag.html": b"<p>a </b",
    "eof-slash.html": b"<p>a </",
    "eof-comment.html": b"<p>a <!-- c",
    "eof-decl.html": b"<p>a <!x",
    "eof-pi.html": b"<p>a <?pi",
    "eof-lt.html": b"<p>a <",
    "eof-cdata.html": b"<p>a <![CDATA[ x",
    "script-open.html": b"<html><body>before<script>never closed <p>hidden</p>",
    "latin1.htm": b"<html><body><p>caf\xe9</p></body></html>",
    "x.rtf": rb"{\rtf1\ansi\ansicpg1251{\fonttbl{\f0 Arial;}}{\*\generator X;}\f0 \'ef\'f0\'e8\'e2\'e5\'f2 @U1087?@U1088?\par line two\line break\tab tab \{brace\} \~nbsp\par\par\par\par end}".replace(b"@U", b"\\u"),
    "emoji.rtf": rb"{\rtf1\ansi Hi \uc1\u-10179?\u-8704? there \'e9 \u233 e\par}",
    "html-in.txt": b"<!doctype html><html><body><h2>Sniffed</h2><p>from content</p></body></html>",
    "rtf-in.dat": rb"{\rtf1\ansi sniffed rtf}",
    "xml.xml": b"<?xml version='1.0'?><a><b>text</b></a>",
}


W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"'
R_NS = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
A_NS = 'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
S_NS = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
REL = 'xmlns="http://schemas.openxmlformats.org/package/2006/relationships"'
O_NS = ('xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
        'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
        'xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:presentation="urn:oasis:names:tc:opendocument:xmlns:presentation:1.0" '
        'xmlns:xlink="http://www.w3.org/1999/xlink"')


def para(text, style=None, num=None, extra=""):
    ppr = (f'<w:pStyle w:val="{style}"/>' if style else "") + (f'<w:numPr><w:ilvl w:val="{num[1]}"/><w:numId w:val="{num[0]}"/></w:numPr>' if num else "") + extra
    return f'<w:p><w:pPr>{ppr}</w:pPr><w:r><w:t xml:space="preserve">{text}</w:t></w:r></w:p>'


def rich_docx():
    body = (para("Title text", "Title") + para("Intro *with* # marks", None) + para("Heading two", "Heading2")
            + para("outline para", None, extra='<w:outlineLvl w:val="2"/>')
            + para("Bullet one", None, ("1", "0")) + para("Bullet child", None, ("1", "1")) + para("Number one", None, ("2", "0"))
            + para("Number two", None, ("2", "0")) + para("Off", None, ("0", "0")) + para("Styled bullet", "ListBullet")
            + para("Styled number", "ListNumber2")
            + '<w:p><w:r><w:t>Tab</w:t><w:tab/><w:t>stop</w:t><w:br/><w:t>break</w:t><w:delText>gone</w:delText></w:r>'
              '<w:hyperlink><w:r><w:t> linked</w:t></w:r></w:hyperlink></w:p>'
            + '<w:tbl><w:tr><w:tc>' + para("A1") + para("A1b") + '</w:tc><w:tc>' + para("B | 1") + '</w:tc></w:tr>'
              '<w:tr><w:tc>' + para("") + '</w:tc><w:tc>' + para("2 * 3") + '</w:tc></w:tr></w:tbl>'
            + '<w:sdt><w:sdtContent>' + para("In content control") + '</w:sdtContent></w:sdt>' + para("1. looks like a list") + para("- dash"))
    styles = (f'<w:styles {W}><w:style w:styleId="Title"><w:name w:val="Title"/></w:style>'
              '<w:style w:styleId="Heading2"><w:name w:val="heading 2"/></w:style>'
              '<w:style w:styleId="ListBullet"><w:name w:val="List Bullet"/></w:style>'
              '<w:style w:styleId="ListNumber2"><w:name w:val="List Number 2"/></w:style></w:styles>')
    numbering = (f'<w:numbering {W}><w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl>'
                 '<w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>'
                 '<w:abstractNum w:abstractNumId="8"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>'
                 '<w:num w:numId="1"><w:abstractNumId w:val="7"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="8"/></w:num></w:numbering>')
    notes = (f'<w:footnotes {W}><w:footnote w:type="separator"><w:p><w:r><w:t>sep</w:t></w:r></w:p></w:footnote>'
             '<w:footnote w:id="1">' + para("Footnote text") + '</w:footnote></w:footnotes>')
    ends = notes.replace("Footnote", "Endnote").replace("footnotes", "endnotes").replace("footnote", "endnote")
    return zipped({"word/document.xml": f'<w:document {W}><w:body>{body}</w:body></w:document>', "word/styles.xml": styles,
                   "word/numbering.xml": numbering, "word/footnotes.xml": notes, "word/endnotes.xml": ends})


def rich_pptx():
    def sp(ph, *paras):
        pr = f'<p:nvSpPr><p:nvPr><p:ph type="{ph}"/></p:nvPr></p:nvSpPr>' if ph else ""
        return f'<p:sp>{pr}<p:txBody>' + "".join(paras) + '</p:txBody></p:sp>'
    ap = lambda t, ppr="": f'<a:p>{ppr}<a:r><a:t>{t}</a:t></a:r></a:p>'
    cell = lambda t: '<a:tc><a:txBody>' + ap(t) + '</a:txBody></a:tc>'
    tree = (sp("title", ap("Deck title")) + sp("body", ap("Point one"), ap("Child", '<a:pPr lvl="1"/>'), ap("Numbered", '<a:pPr><a:buAutoNum/></a:pPr>'),
            ap("No bullet", "<a:pPr><a:buNone/></a:pPr>")) + sp(None, ap("Loose text"), ap("Char bullet", "<a:pPr><a:buChar/></a:pPr>"))
            + '<p:grpSp>' + sp("subTitle", ap("In group")) + '</p:grpSp>'
            + '<p:graphicFrame><a:graphic><a:graphicData><a:tbl><a:tr>' + cell("H") + cell("V") + '</a:tr><a:tr>' + cell("1") + cell("2")
            + '</a:tr></a:tbl></a:graphicData></a:graphic></p:graphicFrame>')
    slide = lambda t: f'<p:sld {A_NS} {R_NS}><p:cSld><p:spTree>{t}</p:spTree></p:cSld></p:sld>'
    notes = f'<p:notes {A_NS}><p:cSld><p:spTree>' + sp("body", ap("Speaker note")) + sp("sldNum", ap("7")) + '</p:spTree></p:cSld></p:notes>'
    return zipped({"[Content_Types].xml": "<Types/>", "ppt/presentation.xml": f"<p:presentation {A_NS}/>",
                   "ppt/slides/slide10.xml": slide(sp("title", ap("Tenth"))), "ppt/slides/slide2.xml": slide(tree),
                   "ppt/slides/_rels/slide2.xml.rels": f'<Relationships {REL}><Relationship Id="rId1" Target="../notesSlides/notesSlide2.xml"/></Relationships>',
                   "ppt/notesSlides/notesSlide2.xml": notes})


def rich_xlsx():
    wb = f'<workbook {S_NS} {R_NS}><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet sheetId="2" r:id="rId2"/><sheet name="Gone" r:id="rId9"/></sheets></workbook>'
    rels = f'<Relationships {REL}><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/></Relationships>'
    sheet1 = (f'<worksheet {S_NS}><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="D1" t="inlineStr"><is><t>inline</t></is></c></row>'
              '<row><c r="B2"><v>3.5</v></c><c r="C2" t="s"><v>99</v></c><c r="E2" t="str"><v>tab\there</v></c></row><row/></sheetData></worksheet>')
    sheet2 = f'<worksheet {S_NS}><sheetData><row><c r="A1"><v>only</v></c></row></sheetData></worksheet>'
    sst = f'<sst {S_NS}><si><t>shared</t></si><si><r><t>rich </t></r><r><t>text</t></r></si></sst>'
    return zipped({"xl/workbook.xml": wb, "xl/_rels/workbook.xml.rels": rels, "xl/worksheets/sheet1.xml": sheet1,
                   "xl/worksheets/sheet2.xml": sheet2, "xl/sharedStrings.xml": sst})


def rich_odt():
    content = (f'<office:document-content {O_NS}><office:automatic-styles><text:list-style style:name="Num"><text:list-level-style-number text:level="1"/>'
               '<text:list-level-style-bullet text:level="2"/></text:list-style></office:automatic-styles>'
               '<office:body><office:text><text:h text:outline-level="1">Top <text:span>head</text:span></text:h>'
               '<text:p>Para <text:span>with</text:span> <text:a>link</text:a> <text:note><text:note-body><text:p>note</text:p></text:note-body></text:note>tail<text:line-break/>next</text:p>'
               '<text:list text:style-name="Num"><text:list-item><text:p>First</text:p><text:list><text:list-item><text:p>Inner</text:p></text:list-item></text:list></text:list-item>'
               '<text:list-item><text:p>Second</text:p></text:list-item></text:list>'
               '<text:p>- not a bullet</text:p><draw:frame><draw:text-box><text:p>Boxed</text:p></draw:text-box></draw:frame>'
               '<table:table><table:table-row><table:table-cell><text:p>x</text:p></table:table-cell><table:covered-table-cell/></table:table-row></table:table>'
               '</office:text></office:body></office:document-content>')
    return zipped({"mimetype": "application/vnd.oasis.opendocument.text", "content.xml": content})


def rich_odp_ods():
    odp = (f'<office:document-content {O_NS}><office:body><office:presentation><draw:page>'
           '<draw:frame presentation:class="title"><draw:text-box><text:p>Slide title</text:p></draw:text-box></draw:frame>'
           '<draw:frame presentation:class="outline"><draw:text-box><text:list><text:list-item><text:p>Bullet</text:p></text:list-item></text:list></draw:text-box></draw:frame>'
           '</draw:page></office:presentation></office:body></office:document-content>')
    ods = (f'<office:document-content {O_NS}><office:body><office:spreadsheet><table:table table:name="S1"><table:table-row>'
           '<table:table-cell table:number-columns-repeated="3"><text:p>r</text:p></table:table-cell><table:table-cell/></table:table-row>'
           '<table:table-row><table:table-cell/><table:table-cell><text:p>b</text:p></table:table-cell></table:table-row></table:table>'
           '<table:table><table:table-row><table:table-cell><text:p>unnamed</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>')
    return zipped({"content.xml": odp}), zipped({"content.xml": ods})


def rich_epub():
    opf = ('<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="a" href="text/one.xhtml"/><item id="b" href="../two.xhtml"/>'
           '<item id="nofile" href="missing.xhtml"/></manifest><spine><itemref idref="a"/><itemref idref="nofile"/><itemref idref="b"/><itemref idref="zzz"/></spine></package>')
    return zipped({"META-INF/container.xml": '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/c.opf"/></rootfiles></container>',
                   "OEBPS/c.opf": opf, "OEBPS/text/one.xhtml": "<html><body><h2>One</h2><p>a<br/>b</p></body></html>", "two.xhtml": "<html><body><p>Two</p></body></html>"})


_odp, _ods = rich_odp_ods()
EXTRA.update({"rich.docx": rich_docx(), "rich.pptx": rich_pptx(), "rich.xlsx": rich_xlsx(), "rich.odt": rich_odt(), "rich.odp": _odp, "rich.ods": _ods,
              "rich.epub": rich_epub(), "guessed.dat": rich_docx(), "guessed2.bin": rich_xlsx()})


def run_python(name, data):
    try:
        return ("ok", json.loads(json.dumps(extract.extract(name, data))))  # as the browser receives it (surrogate pairs join)
    except extract.ExtractError as e:
        return ("error", str(e))
    except Exception as e:  # server.py: "Couldn't read this file (Name)."
        return ("fail", type(e).__name__)


def run_rust(cmd, name, data):
    with tempfile.TemporaryDirectory() as d:
        path = Path(d) / name
        path.write_bytes(data)
        out = subprocess.run(cmd + [str(path)], cwd=RUST, capture_output=True)
    if out.returncode:
        return ("crash", out.stderr.decode("utf-8", "replace")[-300:])
    res = json.loads(out.stdout.decode("utf-8"))
    if isinstance(res, dict):
        msg = res["error"]
        return ("fail", msg) if msg.startswith("Couldn't read this file (") else ("error", msg)
    return ("ok", res)


def first_diff(a, b, path="parts"):
    """Where two JSON values first differ, as a short string; None when equal."""
    if type(a) is not type(b):
        return f"{path}: {a!r:.80} vs {b!r:.80}"
    if isinstance(a, dict):
        for k in sorted(set(a) | set(b)):
            if k not in a or k not in b:
                return f"{path}.{k}: missing in {'python' if k not in a else 'rust'}"
            d = first_diff(a[k], b[k], f"{path}.{k}")
            if d:
                return d
    elif isinstance(a, list):
        for i, (x, y) in enumerate(zip(a, b)):
            d = first_diff(x, y, f"{path}[{i}]")
            if d:
                return d
        if len(a) != len(b):
            return f"{path}: {len(a)} items in python, {len(b)} in rust"
    elif isinstance(a, str) and a != b:
        i = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
        return f"{path} differs at char {i}: python {a[max(0, i - 15):i + 25]!r} / rust {b[max(0, i - 15):i + 25]!r}"
    elif a != b:
        return f"{path}: {a!r:.80} vs {b!r:.80}"
    return None


def compare(py, rs):
    if py[0] in ("ok", "error") and py[0] == rs[0]:
        return first_diff(py[1], rs[1], "parts" if py[0] == "ok" else "error")
    if py[0] == "fail" and rs[0] == "fail":
        return None  # both stop on a malformed file
    return f"python {py[0]} ({str(py[1])[:100]!r}) vs rust {rs[0]} ({str(rs[1])[:100]!r})"


def old_html_parser():
    """html.parser before its 2025 fix (3.12.10 still had the old behaviour, 3.12.13 has the new one) kept unfinished tags at the end of the input as text;
    the Rust reader follows the fixed behaviour, so those files can't match on an older Python."""
    from html.parser import HTMLParser
    out = []
    p = HTMLParser()
    p.handle_data = out.append
    p.feed("a <b")
    p.close()
    return "".join(out) != "a "


def main():
    if not DOCS.is_dir() or not any(DOCS.iterdir()):
        subprocess.run([sys.executable, str(HERE / "make_docs.py")], check=True, stdout=subprocess.DEVNULL)
    cmd = build_example()
    cases = [(f.name, f.read_bytes()) for f in sorted(DOCS.iterdir()) if f.is_file()] + sorted(EXTRA.items())
    bad = skipped = 0
    for name, data in cases:
        py, rs = run_python(name, data), run_rust(cmd, name, data)
        diff = compare(py, rs)
        if diff is not None and name.startswith(("unterminated", "eof-")) and old_html_parser():
            print(f"SKIP {name}: this Python's html.parser predates the 2025 fix ({sys.version.split()[0]})")
            skipped += 1
            continue
        kind = {"ok": f"{len(py[1]) if py[0] == 'ok' else 0} parts", "error": "same error", "fail": "both fail"}[py[0]]
        print(f"OK   {name} ({kind})" if diff is None else f"DIFF {name}: {diff}")
        bad += diff is not None
    print(f"{len(cases)} files: {len(cases) - bad - skipped} OK, {bad} DIFF" + (f", {skipped} skipped" if skipped else ""))
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
