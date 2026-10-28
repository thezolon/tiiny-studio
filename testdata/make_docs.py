"""Build one small test file per format Text mode reads, into testdata/docs/. Every file says "DOC TEST"
and a format name, so it's obviously test data. Usage: python make_docs.py"""
import io
import zipfile
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

OUT = Path(__file__).parent / "docs"
OUT.mkdir(exist_ok=True)
try:
    FONT = ImageFont.truetype("C:/Windows/Fonts/arial.ttf", 44)
except OSError:
    FONT = ImageFont.load_default(44)


def picture(lines, size=(1100, 300)):
    im = Image.new("RGB", size, "white")
    d = ImageDraw.Draw(im)
    for i, line in enumerate(lines):
        d.text((40, 40 + i * 70), line, fill="black", font=FONT)
    return im


def png(lines):
    b = io.BytesIO()
    picture(lines).save(b, "PNG")
    return b.getvalue()


def zipped(path, files):
    with zipfile.ZipFile(OUT / path, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            z.writestr(name, data)


# Word, with a table and an embedded picture of text (python-docx)
import docx  # noqa: E402
d = docx.Document()
d.add_heading("DOC TEST Word document", 1)
d.add_paragraph("This paragraph comes from a DOCX file. It should be read directly, without OCR.")
t = d.add_table(rows=2, cols=2)
for (r, c), v in {(0, 0): "Item", (0, 1): "Price", (1, 0): "Apples", (1, 1): "3.50"}.items():
    t.cell(r, c).text = v
d.add_heading("Shopping list", 2)
d.add_paragraph("Milk", style="List Bullet")
d.add_paragraph("Eggs", style="List Bullet")
d.add_heading("Steps", 2)
d.add_paragraph("Preheat the oven", style="List Number")
d.add_paragraph("Bake for 20 minutes", style="List Number")
d.add_paragraph("Notes with *stars* and # hashes stay literal.")
d.add_paragraph("Below is a picture with text that needs OCR:")
d.add_picture(io.BytesIO(png(["Picture inside the Word file", "Order 4417 ships Monday"])), width=docx.shared.Inches(5))
d.add_paragraph("Last line of the Word document.")
d.save(OUT / "test.docx")

CT = '<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>'
R = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
A = 'xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"'
RELS = 'xmlns="http://schemas.openxmlformats.org/package/2006/relationships"'


def slide(*paras):
    body = "".join(f"<a:p><a:r><a:t>{p}</a:t></a:r></a:p>" for p in paras)
    return f'<p:sld {A} {R}><p:cSld><p:spTree><p:sp><p:txBody>{body}</p:txBody></p:sp>' \
           f'<p:pic><p:blipFill><a:blip r:embed="rId9"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>'


zipped("test.pptx", {
    "[Content_Types].xml": CT, "ppt/presentation.xml": f"<p:presentation {A}/>",
    "ppt/slides/slide1.xml": slide("DOC TEST PowerPoint slide one", "First slide body text"),
    "ppt/slides/slide2.xml": f'<p:sld {A}><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Second slide title</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>',
    "ppt/slides/_rels/slide1.xml.rels": f'<Relationships {RELS}><Relationship Id="rId9" Target="../media/image1.png"/></Relationships>',
    "ppt/media/image1.png": png(["Picture on slide one", "Room 12B at 3 pm"]),
})

S = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"'
zipped("test.xlsx", {
    "[Content_Types].xml": CT,
    "xl/workbook.xml": f'<workbook {S} {R}><sheets><sheet name="Prices" sheetId="1" r:id="rId1"/></sheets></workbook>',
    "xl/_rels/workbook.xml.rels": f'<Relationships {RELS}><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>',
    "xl/sharedStrings.xml": f'<sst {S}><si><t>DOC TEST Excel</t></si><si><t>Pears</t></si></sst>',
    "xl/worksheets/sheet1.xml": f'<worksheet {S}><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row>'
                                f'<row r="2"><c r="A2" t="s"><v>1</v></c><c r="C2"><v>4.25</v></c></row></sheetData></worksheet>',
})

O = ('xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" '
     'xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" '
     'xmlns:xlink="http://www.w3.org/1999/xlink"')
zipped("test.odt", {"mimetype": "application/vnd.oasis.opendocument.text", "content.xml":
    f'<office:document-content {O}><office:body><office:text><text:h>DOC TEST OpenDocument text</text:h>'
    f'<text:p>Words<text:s text:c="2"/>with<text:tab/>spacing.</text:p><text:p><draw:frame><draw:image xlink:href="Pictures/p.png"/></draw:frame></text:p>'
    f'<text:p>After the picture.</text:p><text:h text:outline-level="2">ODT list</text:h><text:list><text:list-item><text:p>Alpha</text:p>'
    f'<text:list><text:list-item><text:p>Alpha child</text:p></text:list-item></text:list></text:list-item><text:list-item><text:p>Beta</text:p></text:list-item></text:list>'
    f'<table:table><table:table-row><table:table-cell><text:p>K</text:p></table:table-cell><table:table-cell><text:p>V</text:p></table:table-cell></table:table-row>'
    f'<table:table-row><table:table-cell><text:p>size</text:p></table:table-cell><table:table-cell><text:p>9</text:p></table:table-cell></table:table-row></table:table>'
    f'</office:text></office:body></office:document-content>',
    "Pictures/p.png": png(["Picture inside the ODT", "Gate 7 closes at noon"])})
zipped("test.ods", {"mimetype": "application/vnd.oasis.opendocument.spreadsheet", "content.xml":
    f'<office:document-content {O}><office:body><office:spreadsheet><table:table table:name="Stock">'
    f'<table:table-row><table:table-cell><text:p>DOC TEST Calc</text:p></table:table-cell><table:table-cell table:number-columns-repeated="1000"/></table:table-row>'
    f'<table:table-row><table:table-cell><text:p>Plums</text:p></table:table-cell><table:table-cell><text:p>12</text:p></table:table-cell></table:table-row>'
    f'</table:table></office:spreadsheet></office:body></office:document-content>'})
zipped("test.odp", {"mimetype": "application/vnd.oasis.opendocument.presentation", "content.xml":
    f'<office:document-content {O}><office:body><office:presentation><draw:page><draw:frame><draw:text-box><text:p>DOC TEST Impress slide</text:p></draw:text-box></draw:frame></draw:page>'
    f'<draw:page><draw:frame><draw:text-box><text:p>Second Impress slide</text:p></draw:text-box></draw:frame></draw:page></office:presentation></office:body></office:document-content>'})

zipped("test.epub", {
    "mimetype": "application/epub+zip",
    "META-INF/container.xml": '<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>',
    "OEBPS/content.opf": '<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="c1" href="ch1.xhtml"/><item id="c2" href="ch2.xhtml"/></manifest>'
                         '<spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>',
    "OEBPS/ch1.xhtml": "<html><head><title>x</title><style>p{}</style></head><body><h1>DOC TEST EPUB chapter one</h1><p>It was a quiet morning.</p></body></html>",
    "OEBPS/ch2.xhtml": "<html><body><h1>Chapter two</h1><p>The end &amp; goodbye.</p></body></html>",
})

(OUT / "test.rtf").write_bytes(rb"{\rtf1\ansi\ansicpg1252{\fonttbl{\f0 Arial;}}{\colortbl;\red0\green0\blue0;}\f0 DOC TEST Rich Text\par Caf\'e9 costs 4\u8364? today.\par\tab Indented line.}")
(OUT / "test.html").write_text("<!doctype html><html><head><script>var x=1</script></head><body><h1>DOC TEST web page</h1><p>Hello&nbsp;there.</p><table><tr><td>A</td><td>B</td></tr></table></body></html>", encoding="utf-8")
(OUT / "test.csv").write_text('Name,Qty,Price\nApples,3,1.20\n"Pears, green",5,0.80\n', encoding="utf-8")
(OUT / "test.json").write_text('{"doc": "DOC TEST json", "items": [1, 2]}', encoding="utf-8")
(OUT / "test.md").write_text("# DOC TEST markdown\n\n- kept as is\n", encoding="utf-8")
(OUT / "test-rich.html").write_text("<html><body><h1>DOC TEST rich page</h1><p>Intro text.</p><h2>Things</h2><ul><li>One<ul><li>One point five</li></ul></li><li>Two</li></ul>"
    "<ol><li>First</li><li>Second</li></ol><table><tr><th>Col A</th><th>Col B</th></tr><tr><td>1</td><td>x | y</td></tr></table><pre>code  line\n  indented</pre></body></html>", encoding="utf-8")
(OUT / "test.txt").write_text("DOC TEST plain text\nSecond line.\n", encoding="utf-8")

# PDFs: one with a real text layer (hand-written PDF), one scanned (pictures only, 2 pages), and a TIFF scan (2 pages)
content = b"BT /F1 24 Tf 72 720 Td (DOC TEST PDF with a text layer) Tj 0 -40 Td (Invoice 9921 is due on Friday.) Tj ET"
objs = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
pdf, offs = b"%PDF-1.4\n", []
for i, o in enumerate(objs, 1):
    offs.append(len(pdf)); pdf += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(pdf)
pdf += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1) + b"".join(b"%010d 00000 n \n" % o for o in offs)
pdf += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
(OUT / "test-text.pdf").write_bytes(pdf)


def text_pdf(lines):
    """A one-page PDF with a real text layer; lines are (font size, text)."""
    ops, y = [b"BT"], 740
    for size, text in lines:
        ops.append(b"/F1 %d Tf 1 0 0 1 72 %d Tm (%s) Tj" % (size, y, text.encode("latin-1")))
        y -= int(size * 1.6)
    stream = b"\n".join(ops + [b"ET"])
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
            b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"\nendstream", b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    out, offs = b"%PDF-1.4\n", []
    for i, o in enumerate(objs, 1):
        offs.append(len(out)); out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1) + b"".join(b"%010d 00000 n \n" % o for o in offs)
    return out + b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)


(OUT / "test-headings.pdf").write_bytes(text_pdf([
    (28, "DOC TEST Quarterly Report"), (11, "This report covers the third quarter. Sales grew in every region and"),
    (11, "costs stayed flat, so margins improved."), (18, "Sales"), (11, "- North region up 12 percent"),
    (11, "- South region up 8 percent"), (18, "Next steps"), (11, "1. Hire two more people"), (11, "2. Open the Lisbon office")]))

pages = [picture(["DOC TEST scanned PDF page one", "The meeting moved to Thursday"], (1275, 400)),
         picture(["Scanned page two", "Bring the blue folder"], (1275, 400))]
pages[0].save(OUT / "test-scanned.pdf", save_all=True, append_images=pages[1:], resolution=150)
tif = [p.convert("L") for p in [picture(["DOC TEST TIFF page one", "Fax from the front desk"]), picture(["TIFF page two", "Call back after five"])]]
tif[0].save(OUT / "test.tiff", save_all=True, append_images=tif[1:], compression="tiff_deflate")
picture(["DOC TEST BMP picture", "Plain bitmap text"]).save(OUT / "test.bmp")
picture(["DOC TEST GIF picture", "Animated formats work too"]).save(OUT / "test.gif")
print("wrote", sorted(p.name for p in OUT.iterdir()))
