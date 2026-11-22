"""Text (and pictures) out of documents, in memory: Office Open XML (docx, pptx, xlsx), OpenDocument
(odt, odp, ods), EPUB, RTF, HTML and plain text. Nothing is written to disk.

extract(name, data) -> list of parts in reading order: {"label", "text", "md"} or {"label", "image"} (a data
URI, for the browser to send through OCR). Documents are read into blocks (headings, paragraphs, list
items, tables) so the same content can be shown as plain text or as Markdown. PDFs and images are read in
the browser instead."""
import base64
import csv
import html.parser
import io
import itertools
import json
import posixpath
import re
import threading
import urllib.parse
import xml.etree.ElementTree as ET
import zipfile

MAX_MEMBER = 64 << 20      # per file inside a zip: guards against zip bombs
MAX_TOTAL = 256 << 20
MAX_IMAGES = 50
MAX_ROWS = 5000            # per sheet or CSV
MIN_IMAGE = 6 << 10        # smaller pictures are icons, bullets and logos
MAX_CELLS = 1_000_000      # cells kept per sheet
MAX_COLS = 16_384          # Excel's own column limit
MAX_LIST_DEPTH = 16        # deeper list nesting / longer runs of spaces in a file are clamped
MAX_SPACES = 1000
_cut = threading.local()   # set when a limit shortened a paragraph while reading; odf() turns it into a visible note


def rows_note(n):
    return f"[… {n} more rows not shown]"
IMAGE_TYPES = {"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg", "gif": "image/gif", "bmp": "image/bmp",
               "webp": "image/webp", "tif": "image/tiff", "tiff": "image/tiff"}
NS = {
    "w": "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
    "a": "http://schemas.openxmlformats.org/drawingml/2006/main",
    "p": "http://schemas.openxmlformats.org/presentationml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
    "s": "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    "rel": "http://schemas.openxmlformats.org/package/2006/relationships",
    "v": "urn:schemas-microsoft-com:vml",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "draw": "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0",
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "style": "urn:oasis:names:tc:opendocument:xmlns:style:1.0",
    "pres": "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0",
    "xlink": "http://www.w3.org/1999/xlink",
    "mc": "http://schemas.openxmlformats.org/markup-compatibility/2006",
}
TEXT_EXT = {"txt", "md", "markdown", "csv", "tsv", "json", "xml", "log", "ini", "yaml", "yml", "srt", "vtt", "py", "js", "css"}
LEGACY = {"doc": "Word 97-2003 (.doc)", "xls": "Excel 97-2003 (.xls)", "ppt": "PowerPoint 97-2003 (.ppt)"}


class ExtractError(Exception):
    pass


def q(tag):
    p, t = tag.split(":")
    return f"{{{NS[p]}}}{t}"


# ---------- blocks -> text / Markdown ----------
# ("h", level, text) · ("p", text) · ("li", depth, ordered, text) · ("table", rows) · ("pre", lang, text)

def md_escape(s):
    """Escape what would otherwise turn plain text into Markdown syntax."""
    s = re.sub(r"([\\`*\[\]<])", r"\\\1", s)
    return re.sub(r"(?m)^(\s*)([#>+-]|\d+[.)])(?=\s|$)", lambda m: m.group(1) + ("\\" + m.group(2) if not m.group(2)[0].isdigit() else m.group(2)[:-1] + "\\" + m.group(2)[-1]), s)


def md_lines(s):
    """Line breaks inside a paragraph become Markdown hard breaks."""
    return "  \n".join(md_escape(line.strip()) for line in s.split("\n") if line.strip())


def md_table(rows):
    width = max(len(r) for r in rows)
    cell = lambda c: md_escape(c).replace("|", "\\|").replace("\n", "<br>").strip() or " "
    rows = [[cell(c) for c in r] + [" "] * (width - len(r)) for r in rows]
    return "\n".join(["| " + " | ".join(rows[0]) + " |", "|" + " --- |" * width] + ["| " + " | ".join(r) + " |" for r in rows[1:]])


def render(blocks):
    text, md, counters = [], [], {}
    prev, prev_ordered = None, None
    for b in blocks:
        kind = b[0]
        if kind != "li":
            counters = {}
        if kind == "h":
            text.append(b[2]); md.append(("\n" if md else "") + "#" * b[1] + " " + md_escape(b[2].replace("\n", " ")))
        elif kind == "p":
            text.append(b[1]); md.append(("\n" if md else "") + md_lines(b[1]))
        elif kind == "li":
            depth, ordered, t = b[1], b[2], b[3]
            counters = {d: n for d, n in counters.items() if d <= depth}
            counters[depth] = counters.get(depth, 0) + 1
            text.append("  " * depth + (f"{counters[depth]}. " if ordered else "• ") + t.replace("\n", " "))
            new_list = prev != "li" or (depth == 0 and ordered != prev_ordered)  # a bullet list then a numbered one
            md.append(("\n" if md and new_list else "") + "   " * depth + (f"{counters[depth]}. " if ordered else "- ") + md_escape(t.replace("\n", " ")))
            if depth == 0:
                prev_ordered = ordered
        elif kind == "table":
            rows = [r for r in b[1] if any(c.strip() for c in r)]
            if rows:
                text.append("\n".join("\t".join(c.replace("\n", " ") for c in r) for r in rows))
                md.append(("\n" if md else "") + md_table(rows))
        elif kind == "pre":
            text.append(b[2]); md.append(("\n" if md else "") + f"```{b[1]}\n{b[2]}\n```")
        prev = kind
    return "\n".join(text).strip(), "\n".join(md).strip()


class Doc:
    """Collects blocks and pictures in reading order, split into parts at each picture."""
    def __init__(self, zf=None):
        self.zf, self.parts, self.blocks, self.images, self.total = zf, [], [], 0, 0

    def read(self, name):
        info = self.zf.getinfo(name)
        limit = min(MAX_MEMBER, MAX_TOTAL - self.total)
        if info.file_size > limit:
            raise ExtractError("The document is too large to read.")
        with self.zf.open(info) as f:
            data = f.read(limit + 1)  # the limits count the bytes really read, not the size the directory claims
        if len(data) > limit:
            raise ExtractError("The document is too large to read.")
        self.total += len(data)
        return data

    def xml(self, name):
        return ET.fromstring(self.read(name))

    def has(self, name):
        return name in self.zf.namelist()

    def add(self, *block):
        if block[0] == "h":
            block = ("h", min(max(block[1], 0), 6), block[2])
        elif block[0] == "li":
            block = ("li", min(max(block[1], 0), MAX_LIST_DEPTH), block[2], block[3])
        last = block[-1]
        if block[0] == "table" or (isinstance(last, str) and last.strip()):
            self.blocks.append(block)

    def flush(self, label):
        text, md = render(self.blocks)
        if text:
            self.parts.append({"label": label, "text": text, "md": md})
        self.blocks = []

    def image(self, name, label):
        ext = name.rsplit(".", 1)[-1].lower()
        if ext not in IMAGE_TYPES or self.images >= MAX_IMAGES or not self.has(name) or self.zf.getinfo(name).file_size < MIN_IMAGE:
            return
        self.flush(label)
        self.images += 1
        self.parts.append({"label": f"{label}, picture {self.images}",
                           "image": f"data:{IMAGE_TYPES[ext]};base64," + base64.b64encode(self.read(name)).decode()})


def rels(doc, part):
    """Relationship id -> target path for an OOXML part."""
    d, f = posixpath.split(part)
    path = posixpath.join(d, "_rels", f + ".rels")
    if not doc.has(path):
        return {}
    out = {}
    for r in doc.xml(path).iter(q("rel:Relationship")):
        if r.get("TargetMode") != "External":
            # an absolute target ("/xl/worksheets/sheet1.xml") names the zip member without its slash
            out[r.get("Id")] = posixpath.normpath(posixpath.join(d, r.get("Target", ""))).lstrip("/")
    return out


def ooxml_runs(el, pics):
    """Text of a paragraph (runs, tabs, breaks, hyperlinks); pictures found inside go to pics."""
    out = []
    for node in el:
        t = node.tag
        if t in (q("w:t"), q("a:t")):
            out.append(node.text or "")
        elif t == q("w:tab"):
            out.append("\t")
        elif t in (q("w:br"), q("w:cr"), q("a:br")):
            out.append("\n")
        elif t in (q("a:blip"), q("v:imagedata")):
            rid = node.get(q("r:embed")) or node.get(q("r:id"))
            if rid:
                pics.append(rid)
        elif t == q("w:p"):  # a paragraph inside a paragraph (a text box): kept apart from the text around it
            out.append("\n" + ooxml_runs(node, pics) + "\n")
        elif t not in (q("w:delText"), q("w:instrText"), q("w:pPr"), q("a:pPr"), q("mc:Fallback")):
            out.append(ooxml_runs(node, pics))
    return "".join(out)


# ---------- Word ----------
def docx_styles(doc):
    """Paragraph style id -> heading level (from the style name or outline level), and -> list numId."""
    heads, lists = {}, {}
    if not doc.has("word/styles.xml"):
        return heads, lists
    for st in doc.xml("word/styles.xml").iter(q("w:style")):
        sid, name = st.get(q("w:styleId")), ((st.find(q("w:name")).get(q("w:val")) or "") if st.find(q("w:name")) is not None else "")
        m = re.fullmatch(r"heading (\d)", name.lower())
        lvl = st.find(f"{q('w:pPr')}/{q('w:outlineLvl')}")
        if m:
            heads[sid] = int(m.group(1))
        elif name.lower() == "title":
            heads[sid] = 1
        elif lvl is not None and lvl.get(q("w:val"), "9").isdigit() and int(lvl.get(q("w:val"))) < 6:
            heads[sid] = int(lvl.get(q("w:val"))) + 1
        num = st.find(f"{q('w:pPr')}/{q('w:numPr')}/{q('w:numId')}")
        if num is not None:
            lists[sid] = num.get(q("w:val"))
        elif re.fullmatch(r"list (bullet|number)( \d)?", name.lower()):
            lists[sid] = "bullet" if "bullet" in name.lower() else "number"
    return heads, lists


def docx_numbering(doc):
    """(numId, level) -> True when that list level is numbered rather than bulleted."""
    if not doc.has("word/numbering.xml"):
        return {}
    root = doc.xml("word/numbering.xml")
    abstract = {}
    for a in root.iter(q("w:abstractNum")):
        for lvl in a.iter(q("w:lvl")):
            fmt = lvl.find(q("w:numFmt"))
            abstract[(a.get(q("w:abstractNumId")), lvl.get(q("w:ilvl")))] = fmt is not None and fmt.get(q("w:val")) not in ("bullet", "none")
    out = {}
    for n in root.iter(q("w:num")):
        aid = n.find(q("w:abstractNumId"))
        if aid is not None:
            for (a, ilvl), ordered in abstract.items():
                if a == aid.get(q("w:val")):
                    out[(n.get(q("w:numId")), ilvl)] = ordered
    return out


def table_rows(tbl):
    """The rows of a table, through content controls (w:sdt) but not into tables nested in a cell."""
    out = []
    for k in tbl:
        if k.tag == q("w:tr"):
            out.append(k)
        elif k.tag == q("w:sdt") and k.find(q("w:sdtContent")) is not None:
            out += table_rows(k.find(q("w:sdtContent")))
    return out


def table_cells(tr):
    """The cells of a row, also those inside a content control."""
    out = []
    for k in tr:
        if k.tag == q("w:tc"):
            out.append(k)
        elif k.tag == q("w:sdt") and k.find(q("w:sdtContent")) is not None:
            out += table_cells(k.find(q("w:sdtContent")))
    return out


def docx_body(doc, root, rel, label, heads, lists, numbering):
    for el in root:
        t = el.tag
        if t == q("w:p"):
            pics = []
            text = ooxml_runs(el, pics)
            ppr = el.find(q("w:pPr"))
            style = ppr.find(q("w:pStyle")).get(q("w:val")) if ppr is not None and ppr.find(q("w:pStyle")) is not None else None
            num = ppr.find(q("w:numPr")) if ppr is not None else None
            olvl = ppr.find(q("w:outlineLvl")) if ppr is not None else None
            ov = olvl.get(q("w:val"), "") if olvl is not None else ""
            if style in heads:  # a style that is a heading but names no level: the paragraph's own outline level
                level = heads[style] or (int(ov) + 1 if ov.isdigit() else None)
            else:
                level = int(ov) + 1 if ov.isdigit() and int(ov) < 6 else None
            if level is not None:
                doc.add("h", level, text.strip())
            elif num is not None or style in lists:
                ilvl = num.find(q("w:ilvl")) if num is not None else None
                nid = num.find(q("w:numId")) if num is not None else None
                depth = ilvl.get(q("w:val"), "0") if ilvl is not None else "0"
                nid = nid.get(q("w:val")) if nid is not None else lists.get(style)
                if nid == "0":  # numbering switched off for this paragraph
                    doc.add("p", text)
                else:
                    ordered = nid == "number" or numbering.get((nid, depth), False)
                    doc.add("li", int(depth) if depth.isdigit() else 0, ordered, text.strip())
            else:
                doc.add("p", text)
            for rid in pics:
                if rid in rel:
                    doc.image(rel[rid], label)
        elif t == q("w:tbl"):
            rows, pics = [], []
            for tr in table_rows(el):
                rows.append([" ".join(ooxml_runs(p, pics).strip() for p in tc.iter(q("w:p"))).strip() for tc in table_cells(tr)])
            doc.add("table", rows)
            for rid in pics:
                if rid in rel:
                    doc.image(rel[rid], label)
        elif t in (q("w:sdt"), q("w:sdtContent"), q("w:customXml"), q("w:body")):
            docx_body(doc, el, rel, label, heads, lists, numbering)


def docx(doc):
    heads, lists = docx_styles(doc)
    numbering = docx_numbering(doc)
    body = doc.xml("word/document.xml").find(q("w:body"))
    docx_body(doc, body, rels(doc, "word/document.xml"), "Document", heads, lists, numbering)
    doc.flush("Document")
    for name in sorted(n for n in doc.zf.namelist() if re.fullmatch(r"word/(footnotes|endnotes)\.xml", n)):
        root = doc.xml(name)
        for note in root:
            if note.get(q("w:type")) in (None, "normal"):
                docx_body(doc, note, rels(doc, name), "Notes", heads, lists, numbering)
        doc.flush("Notes")


# ---------- PowerPoint ----------
def numbered(names, pattern):
    return sorted((n for n in names if re.fullmatch(pattern, n)), key=lambda n: int(re.search(r"(\d+)\.xml$", n).group(1)))


def pptx_shapes(doc, tree, rel, label):
    for el in tree:
        t = el.tag
        if t == q("p:grpSp"):
            pptx_shapes(doc, el, rel, label)
        elif t == q("p:sp"):
            ph = el.find(f"{q('p:nvSpPr')}/{q('p:nvPr')}/{q('p:ph')}")
            kind = ph.get("type", "body") if ph is not None else None
            body = el.find(q("p:txBody"))
            if body is None:
                continue
            for para in body.findall(q("a:p")):
                pics = []
                text = ooxml_runs(para, pics).strip()
                ppr = para.find(q("a:pPr"))
                depth = int(ppr.get("lvl", "0")) if ppr is not None else 0
                bullet_off = ppr is not None and ppr.find(q("a:buNone")) is not None
                auto = ppr is not None and ppr.find(q("a:buAutoNum")) is not None
                char = ppr is not None and ppr.find(q("a:buChar")) is not None
                if kind in ("title", "ctrTitle"):
                    doc.add("h", 1, text)
                elif not bullet_off and (auto or char or kind in ("body", "obj")):
                    doc.add("li", depth, auto, text)
                else:
                    doc.add("p", text)
        elif t == q("p:graphicFrame"):
            for tbl in el.iter(q("a:tbl")):
                doc.add("table", [[" ".join(ooxml_runs(p, []).strip() for p in tc.iter(q("a:p"))).strip() for tc in tr.findall(q("a:tc"))]
                                  for tr in tbl.findall(q("a:tr"))])
        elif t == q("p:pic"):
            blip = el.find(f".//{q('a:blip')}")
            rid = blip.get(q("r:embed")) if blip is not None else None
            if rid in rel:
                doc.image(rel[rid], label)


def slide_names(doc):
    """The slide files in the deck's own order (p:sldIdLst); slides it doesn't list follow in file-number order."""
    files = numbered(doc.zf.namelist(), r"ppt/slides/slide\d+\.xml")
    if not doc.has("ppt/presentation.xml"):
        return files
    rel = rels(doc, "ppt/presentation.xml")
    try:
        root = doc.xml("ppt/presentation.xml")
    except ET.ParseError:
        return files
    out = []
    for e in root.iter(q("p:sldId")):
        t = rel.get(e.get(q("r:id")))
        if t in files and t not in out:
            out.append(t)
    return out + [f for f in files if f not in out]


def pptx(doc):
    for i, name in enumerate(slide_names(doc), 1):
        rel = rels(doc, name)
        tree = doc.xml(name).find(f"{q('p:cSld')}/{q('p:spTree')}")
        if tree is not None:
            pptx_shapes(doc, tree, rel, f"Slide {i}")
        doc.flush(f"Slide {i}")
        for note in rel.values():
            if re.fullmatch(r"ppt/notesSlides/notesSlide\d+\.xml", note) and doc.has(note):
                for sp in doc.xml(note).iter(q("p:sp")):
                    ph = sp.find(f"{q('p:nvSpPr')}/{q('p:nvPr')}/{q('p:ph')}")
                    if ph is not None and ph.get("type") == "body":
                        for para in sp.iter(q("a:p")):
                            doc.add("p", ooxml_runs(para, []))
                doc.flush(f"Slide {i} notes")


# ---------- Excel ----------
def col_index(ref):
    """Column number (0-based) of a cell reference like "AB12": None when it has no letters (the cell then
    follows the previous one), False beyond Excel's last column (the cell is ignored)."""
    m = re.match(r"[A-Z]+", ref)
    if not m:
        return None
    n = 0
    for ch in m.group(0):
        n = min(n * 26 + ord(ch) - 64, MAX_COLS + 1)
    return n - 1 if n <= MAX_COLS else False


def string_item(si):
    """Text of a shared or inline string: its t and the t of its runs, not the phonetic guide (rPh)."""
    out = []
    for k in si:
        if k.tag == q("s:t"):
            out.append(k.text or "")
        elif k.tag == q("s:r"):
            out += [t.text or "" for t in k.findall(q("s:t"))]
    return "".join(out)


def xlsx(doc):
    shared = []
    if doc.has("xl/sharedStrings.xml"):
        for si in doc.xml("xl/sharedStrings.xml").iter(q("s:si")):
            shared.append(string_item(si))
    rel = rels(doc, "xl/workbook.xml")
    for sheet in doc.xml("xl/workbook.xml").iter(q("s:sheet")):
        target = rel.get(sheet.get(q("r:id")))
        if not target or not doc.has(target):
            continue
        rows, cell_count, rows_left, cols_cut = [], 0, 0, 0
        row_els = list(doc.xml(target).iter(q("s:row")))
        for ri, row in enumerate(row_els):
            cells, next_col = {}, 0  # a cell without a reference goes after the previous one
            for c in row.iter(q("s:c")):
                kind, v = c.get("t"), c.find(q("s:v"))
                if kind == "s" and v is not None and v.text and v.text.isdigit() and int(v.text) < len(shared):
                    val = shared[int(v.text)]
                elif kind == "inlineStr":
                    val = string_item(c.find(q("s:is"))) if c.find(q("s:is")) is not None else "".join(t.text or "" for t in c.iter(q("s:t")))
                else:
                    val = v.text if v is not None and v.text else ""
                col = col_index(c.get("r")) if c.get("r") else None
                col = next_col if col is None else col
                if col is False:
                    cols_cut += bool(val)
                else:
                    next_col = col + 1
                    if val and col < MAX_COLS:
                        cells[col] = val.replace("\t", " ")
                    elif val:
                        cols_cut += 1
            if cells:
                cell_count += max(cells) + 1
                rows.append([cells.get(i, "") for i in range(max(cells) + 1)])
            if len(rows) >= MAX_ROWS or cell_count >= MAX_CELLS:
                rows_left = len(row_els) - ri - 1
                break
        if rows:
            doc.add("table", rows)
        if rows_left > 0:
            doc.add("p", rows_note(rows_left))
        if cols_cut:
            doc.add("p", f"[… {cols_cut} cells beyond column {MAX_COLS} not shown]")
        doc.flush(sheet.get("name") or "Sheet")


# ---------- OpenDocument ----------
def odf_inline(el):
    """Text of an ODF paragraph, with its spaces, tabs and line breaks (notes and frames left out)."""
    out = [el.text or ""]
    for c in el:
        if c.tag == q("text:s"):
            n = int(c.get(q("text:c"), "1"))
            if n > MAX_SPACES:
                _cut.v = True
            out.append(" " * min(max(n, 0), MAX_SPACES))
        elif c.tag == q("text:tab"):
            out.append("\t")
        elif c.tag == q("text:line-break"):
            out.append("\n")
        elif c.tag not in (q("text:note"), q("draw:frame"), q("draw:a")):
            out += odf_inline(c)
        out.append(c.tail or "")
    return out


def odf_list_styles(doc):
    """List style name -> {level: numbered?} from content.xml and styles.xml."""
    out = {}
    for name in ("styles.xml", "content.xml"):
        if doc.has(name):
            for ls in doc.xml(name).iter(q("text:list-style")):
                out[ls.get(q("style:name"))] = {int(l.get(q("text:level"), "1")) - 1: l.tag == q("text:list-level-style-number") for l in ls}
    return out


def odf_images(doc, el, label):
    for img in el.iter(q("draw:image")):
        doc.image(img.get(q("xlink:href"), ""), label)


def odf_walk(doc, el, label, styles, depth=-1, style=None):
    for c in el:
        t = c.tag
        if t == q("text:h"):
            doc.add("h", min(6, int(c.get(q("text:outline-level"), "1"))), "".join(odf_inline(c)).strip())
            odf_images(doc, c, label)
        elif t == q("text:p"):
            doc.add("p", "".join(odf_inline(c)))
            boxes = list(c.iter(q("draw:text-box")))  # a text box anchored in the paragraph: its text follows
            if boxes:
                odf_walk(doc, boxes, label, styles, depth, style)
            odf_images(doc, c, label)
        elif t == q("text:list"):
            odf_walk(doc, c, label, styles, depth + 1, c.get(q("text:style-name")) or style)
        elif t in (q("text:list-item"), q("text:list-header")):
            for item in c:
                if item.tag in (q("text:p"), q("text:h")):
                    doc.add("li", max(depth, 0), styles.get(style, {}).get(max(depth, 0), False), "".join(odf_inline(item)).strip())
                    odf_images(doc, item, label)
                else:
                    odf_walk(doc, [item], label, styles, depth, style)
        elif t == q("table:table"):
            rows = []
            for row in c.iter(q("table:table-row")):
                rows.append([" ".join("".join(odf_inline(p)).strip() for p in cell.iter(q("text:p"))) for cell in row
                             if cell.tag in (q("table:table-cell"), q("table:covered-table-cell"))])
            doc.add("table", rows)
            odf_images(doc, c, label)
        elif t == q("draw:frame") or t == q("draw:image"):
            if c.find(f".//{q('draw:text-box')}") is not None:
                odf_walk(doc, c.iter(q("draw:text-box")), label, styles, depth, style)
            odf_images(doc, c, label)
        else:
            odf_walk(doc, c, label, styles, depth, style)


def odf_cut_note(doc):
    if getattr(_cut, "v", False):
        _cut.v = False
        doc.add("p", "[… long runs of spaces or very long paragraphs were shortened]")


def odf(doc):
    _cut.v = False
    body = doc.xml("content.xml").find(q("office:body"))
    if body is None:
        return
    styles = odf_list_styles(doc)
    pres, sheets = body.find(q("office:presentation")), body.find(q("office:spreadsheet"))
    if pres is not None:
        for i, page in enumerate(pres.iter(q("draw:page")), 1):
            for frame in page.iter(q("draw:frame")):
                cls = frame.get(q("pres:class"))
                if cls == "title":
                    doc.add("h", 1, " ".join("".join(odf_inline(p)).strip() for p in frame.iter(q("text:p"))))
                elif cls == "outline":
                    for box in frame.iter(q("draw:text-box")):
                        odf_walk(doc, box, f"Slide {i}", {k: v for k, v in styles.items()}, -1, None)
                else:
                    odf_walk(doc, [frame], f"Slide {i}", styles)
            odf_cut_note(doc)
            doc.flush(f"Slide {i}")
    elif sheets is not None:
        for t in sheets.iter(q("table:table")):
            rows, cell_count, rows_left, repeat_cut, cols_cut = [], 0, 0, False, False
            row_els = list(t.iter(q("table:table-row")))
            for ri, row in enumerate(row_els):
                cells, gap_cut = [], False
                for c in row:
                    if c.tag not in (q("table:table-cell"), q("table:covered-table-cell")):
                        continue
                    val = " ".join("".join(odf_inline(p)) for p in c.iter(q("text:p")))
                    # a gap of empty cells is one cell repeated: keep it, or what follows lands in the wrong column
                    wanted = int(c.get(q("table:number-columns-repeated"), "1"))
                    n = min(wanted, 256 if not val else 50)
                    if wanted > n:
                        if val:
                            repeat_cut = True
                        else:
                            gap_cut = True
                    if val and gap_cut:
                        repeat_cut = True  # something follows a gap that was shortened
                    if val and n > MAX_COLS - len(cells):
                        cols_cut = True
                    cells += [val] * max(min(n, MAX_COLS - len(cells)), 0)
                while cells and not cells[-1]:
                    cells.pop()
                if cells:
                    cell_count += len(cells)
                    rows.append(cells)
                if len(rows) >= MAX_ROWS or cell_count >= MAX_CELLS:
                    rows_left = len(row_els) - ri - 1
                    break
            if rows:
                doc.add("table", rows)
            if rows_left > 0:
                doc.add("p", rows_note(rows_left))
            if repeat_cut:
                doc.add("p", "[… some repeated cells were shortened]")
            if cols_cut:
                doc.add("p", f"[… cells beyond column {MAX_COLS} not shown]")
            odf_cut_note(doc)
            doc.flush(t.get(q("table:name")) or "Sheet")
    else:
        odf_walk(doc, body, "Document", styles)
        odf_cut_note(doc)
        doc.flush("Document")


# ---------- HTML and EPUB ----------
class HTMLBlocks(html.parser.HTMLParser):
    """HTML into blocks: headings, paragraphs, (nested) list items, tables and preformatted text."""
    BLOCK = {"p", "div", "section", "article", "blockquote", "header", "footer", "main", "aside", "nav", "figure",
             "figcaption", "dd", "dt", "address", "hr", "title", "body"}
    SKIP = {"script", "style", "head", "noscript", "svg", "template"}
    HEAD_TAGS = {"head", "html", "title", "meta", "link", "base", "style", "script", "noscript", "template"}

    def __init__(self, doc):
        super().__init__(convert_charrefs=True)
        self.doc, self.buf, self.kind, self.skip, self.lists, self.table, self.row, self.cell, self.pre = doc, [], "p", 0, [], None, None, None, 0
        self.outer, self.head = [], False  # tables around the one being read; a <head> is open (counts once in skip)

    def cell_break(self):
        """Inside a table cell a block element starts a new line of text there (the cell keeps it as a space)."""
        if self.cell is not None:
            self.cell.append(" ")

    def emit(self):
        text = "".join(self.buf)
        if not self.pre:
            text = re.sub(r"[ \t]*\n[ \t]*", "\n", re.sub(r"[ \t\r\f\v]+", " ", text)).strip()
        if self.kind == "pre":
            self.doc.add("pre", "", text.strip("\n"))
        elif self.kind.startswith("h"):
            self.doc.add("h", int(self.kind[1]), text.replace("\n", " "))
        elif self.kind == "li":
            self.doc.add("li", max(len(self.lists) - 1, 0), bool(self.lists and self.lists[-1]), text.replace("\n", " "))
        else:
            self.doc.add("p", text)
        self.buf, self.kind = [], "p"

    def handle_starttag(self, tag, attrs):
        if self.head and self.skip == 1 and tag not in self.HEAD_TAGS:
            self.head, self.skip = False, 0  # an implied </head>: its end tag is optional
        if tag == "head" and not self.skip:
            self.head = True
            self.skip += 1
        elif tag in self.SKIP:
            self.skip += 1
        elif self.skip:
            return
        elif tag == "br":
            (self.cell if self.cell is not None else self.buf).append("\n")
        elif self.table is not None and tag in ("tr", "td", "th"):
            if tag == "tr":
                self.row = []; self.table.append(self.row)
            else:
                if self.row is None:
                    self.row = []; self.table.append(self.row)
                self.cell = []; self.row.append(self.cell)
        elif tag == "table":
            if self.table is None:
                self.emit()
            else:
                self.outer.append((self.table, self.row, self.cell))
            self.table, self.row, self.cell = [], None, None
        elif tag in ("ul", "ol"):
            self.cell_break(); self.emit(); self.lists.append(tag == "ol")
        elif tag == "li" or tag == "pre" or re.fullmatch(r"h[1-6]", tag):
            self.cell_break(); self.emit(); self.kind = tag
            self.pre += tag == "pre"
        elif tag in self.BLOCK:
            self.cell_break(); self.emit()

    def handle_endtag(self, tag):
        if tag == "head":
            if self.head:
                self.head = False
                self.skip = max(0, self.skip - 1)
        elif tag in self.SKIP:
            self.skip = max(0, self.skip - 1)
        elif self.skip:
            return
        elif tag in ("td", "th"):
            self.cell = None
        elif tag == "table" and self.table is not None:
            rows = [r for r in ([re.sub(r"\s+", " ", "".join(c)).strip() for c in r] for r in self.table) if r]
            if self.outer:  # a table inside a cell: its text becomes part of that cell
                self.table, self.row, self.cell = self.outer.pop()
                flat = " ".join(c for r in rows for c in r if c)
                (self.cell if self.cell is not None else self.buf).append(f" {flat} ")
            else:
                self.table, self.row, self.cell = None, None, None
                self.doc.add("table", rows)
        elif tag in ("ul", "ol"):
            self.cell_break(); self.emit()
            if self.lists:
                self.lists.pop()
        elif tag == "pre":
            self.emit(); self.pre = max(0, self.pre - 1)
        elif tag == "li" or re.fullmatch(r"h[1-6]", tag) or tag in self.BLOCK:
            self.cell_break(); self.emit()

    def handle_data(self, data):
        if not self.skip:
            (self.cell if self.cell is not None else self.buf).append(data)

    def close(self):
        super().close()
        self.emit()


def page_text(raw):
    """Like decode, but a page that isn't UTF-8 and names its charset (<meta charset=...>) is read in that one."""
    if raw[:2] not in (b"\xff\xfe", b"\xfe\xff"):
        try:
            raw.decode("utf-8")
        except UnicodeDecodeError:
            m = re.search(r"charset\s*=\s*[\"']*([A-Za-z0-9_:.-]+)", raw[:2048].decode("latin-1").lower())
            label = m.group(1) if m else ""
            if label in ("iso-8859-1", "latin1", "iso8859-1", "l1", "us-ascii", "ascii"):
                label = "cp1252"
            if label and not label.startswith(("utf-8", "utf8", "utf-16", "utf16")):
                try:
                    return raw.decode(label, "replace")
                except LookupError:
                    pass
    return decode(raw)


def html_blocks(doc, raw):
    p = HTMLBlocks(doc)
    p.feed(page_text(raw))
    p.close()


def epub(doc):
    container = doc.xml("META-INF/container.xml")
    rootfile = next((e.get("full-path") for e in container.iter() if e.tag.endswith("rootfile")), None)
    if not rootfile or not doc.has(rootfile):
        raise ExtractError("This EPUB has no readable contents list.")
    opf, base = doc.xml(rootfile), posixpath.dirname(rootfile)
    items = {e.get("id"): e.get("href") for e in opf.iter() if e.tag.endswith("}item")}
    spine = [e.get("idref") for e in opf.iter() if e.tag.endswith("}itemref")]
    for i, idref in enumerate(spine, 1):
        href = items.get(idref)
        # a manifest href is a URL: no fragment, percent-escapes undone
        path = posixpath.normpath(posixpath.join(base, urllib.parse.unquote(href.split("#")[0]))) if href else None
        if path and doc.has(path):
            html_blocks(doc, doc.read(path))
            doc.flush(f"Section {i}")


# ---------- RTF and plain text ----------
RTF_SYMBOLS = {"lquote": "\u2018", "rquote": "\u2019", "ldblquote": "\u201c", "rdblquote": "\u201d", "endash": "\u2013",
               "emdash": "\u2014", "bullet": "\u2022", "emspace": " ", "enspace": " ", "qmspace": " ", "zwj": "\u200d",
               "zwnj": "\u200c"}


def rtf_text(raw):
    """Plain text from RTF: paragraphs, tabs, \\'hh and \\uN characters; skips font tables, pictures and other
    destinations."""
    s = raw.decode("latin-1")
    out, stack, skip, uc, pending_skip = [], [], False, 1, 0
    codepage = "cp1252"
    for m in re.finditer(r"\\([a-z]{1,32})(-?\d{1,10})? ?|\\'([0-9a-f]{2})|\\([^a-z])|([{}])|[\r\n]+|([^\\{}\r\n]+)", s, re.I):
        word, arg, hexch, sym, brace, plain = m.groups()
        if brace == "{":
            stack.append((skip, uc))
        elif brace == "}":
            skip, uc = stack.pop() if stack else (False, 1)
        elif sym:
            if sym == "*":
                skip = True
            elif not skip and sym in "\\{}":
                out.append(sym)
            elif not skip and sym == "~":
                out.append("\u00a0")
        elif word:
            w = word.lower()
            if w in ("fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "footer", "object", "themedata", "datastore", "listtable", "listoverridetable", "rsidtbl", "generator"):
                skip = True
            elif w == "ansicpg" and arg:
                codepage = f"cp{arg}"
            elif w == "uc" and arg:
                uc = int(arg)
            elif skip:
                pass
            elif w in ("par", "line", "sect", "page", "row"):
                out.append("\n")
            elif w in ("tab", "cell"):
                out.append("\t")
            elif w in RTF_SYMBOLS:
                out.append(RTF_SYMBOLS[w])
            elif w == "u" and arg:
                out.append(chr(int(arg) % 65536))
                pending_skip = uc
        elif hexch:
            if pending_skip:
                pending_skip -= 1
            elif not skip:
                try:
                    out.append(bytes([int(hexch, 16)]).decode(codepage))
                except (LookupError, UnicodeDecodeError):
                    out.append(bytes([int(hexch, 16)]).decode("cp1252", "replace"))
        elif plain and not skip:
            if pending_skip:
                drop = min(pending_skip, len(plain))
                plain, pending_skip = plain[drop:], pending_skip - drop
            out.append(plain)
    return re.sub(r"\n{3,}", "\n\n", "".join(out)).strip()


def utf16_without_bom(raw):
    """Whether BOM-less bytes look like UTF-16: a good share of the sampled bytes are NUL, nearly all of them in
    the high byte of each pair. None when not; True for big-endian. Latin-1 text has no NULs, however its bytes pair up."""
    sample = raw[:min(len(raw), 4096) & ~1]
    even, odd = sample[0::2].count(0), sample[1::2].count(0)
    pairs = len(sample) // 2
    if not pairs or even + odd < pairs // 4:
        return None
    if odd >= even * 4:
        return False
    if even >= odd * 4:
        return True
    return None


def decode(raw):
    bom16 = raw[:2] in (b"\xff\xfe", b"\xfe\xff")
    if not bom16:
        try:
            return raw.decode("utf-8-sig")
        except UnicodeDecodeError:
            pass
    guess = None if bom16 else utf16_without_bom(raw)
    if len(raw) % 2 == 0 and (bom16 or guess is not None):
        try:
            return raw.decode("utf-16" if bom16 else "utf-16-be" if guess else "utf-16-le")
        except UnicodeDecodeError:
            pass
    return raw.decode("cp1252", "replace")


def text_blocks(doc, text):
    """Plain text: paragraphs split at blank lines, line breaks kept."""
    for para in re.split(r"\n\s*\n", text.replace("\r\n", "\n").replace("\r", "\n")):
        doc.add("p", para.strip("\n"))


ZIP_KINDS = {"docx": docx, "docm": docx, "dotx": docx, "pptx": pptx, "pptm": pptx, "ppsx": pptx, "xlsx": xlsx, "xlsm": xlsx,
             "odt": odf, "ott": odf, "odp": odf, "ods": odf, "epub": epub}


def extract(name, data):
    parts = read_parts(name, data)
    for p in parts:
        for k in ("text", "md"):
            if k in p:
                p[k] = p[k].replace("\r\n", "\n").replace("\r", "\n")
    return [p for p in parts if p.get("image") or p.get("text")]


def read_parts(name, data):
    ext = (name.rsplit(".", 1)[-1] if "." in name else "").lower()
    if ext in LEGACY:
        raise ExtractError(f"{LEGACY[ext]} is an old binary format the studio can't read. Save it as the newer format (.{ext}x) or as PDF first.")
    if ext in ZIP_KINDS or data[:4] == b"PK\x03\x04":
        try:
            zf = zipfile.ZipFile(io.BytesIO(data))
        except zipfile.BadZipFile:
            raise ExtractError("This file looks damaged (not a valid document).") from None
        doc = Doc(zf)
        kind = ZIP_KINDS.get(ext)
        if kind is None:  # guess from the contents
            n = set(zf.namelist())
            kind = docx if "word/document.xml" in n else pptx if "ppt/presentation.xml" in n else xlsx if "xl/workbook.xml" in n \
                else odf if "content.xml" in n else epub if "META-INF/container.xml" in n else None
            if kind is None:
                raise ExtractError("That zip file isn't a document the studio can read.")
        try:
            kind(doc)
        except KeyError as e:
            raise ExtractError(f"This document is missing a part it needs ({e}).") from e
        except ET.ParseError:
            raise ExtractError("This document's contents are damaged.") from None
        return doc.parts
    doc = Doc()
    if ext == "rtf" or data[:5] == b"{\\rtf":
        text_blocks(doc, rtf_text(data))
        doc.flush("Document")
    elif ext in ("html", "htm", "xhtml") or re.match(rb"\s*<(!doctype html|html)", data[:200], re.I):
        html_blocks(doc, data)
        doc.flush("Page")
    elif ext in ("md", "markdown"):
        text = decode(data).strip()
        return [{"label": "Text", "text": text, "md": text}]
    elif ext in ("csv", "tsv"):
        text = decode(data).replace("\r\n", "\n").replace("\r", "\n")
        try:
            dialect = "excel-tab" if ext == "tsv" else csv.Sniffer().sniff(text[:4096], ",;\t|")
        except csv.Error:
            dialect = "excel"
        try:
            reader = csv.reader(io.StringIO(text), dialect)
            doc.add("table", list(itertools.islice(reader, MAX_ROWS)))
            if len(doc.blocks[-1][1]) == MAX_ROWS and next(reader, None) is not None:  # one more row pulled, as before
                doc.add("p", "[… more rows not shown]")
        except csv.Error:  # an unclosed quote swallows the rest, or a cell is over the field limit: show it as text
            text_blocks(doc, text.strip())
        doc.flush("Table")
    elif ext == "json":
        text = decode(data).strip()
        try:
            text = json.dumps(json.loads(text), indent=2, ensure_ascii=False)
        except ValueError:
            pass
        doc.add("pre", "json", text)
        doc.flush("Text")
    elif ext in TEXT_EXT or b"\x00" not in data[:4096]:
        text_blocks(doc, decode(data).strip())
        doc.flush("Text")
    else:
        raise ExtractError("The studio can't read this kind of file.")
    return doc.parts
