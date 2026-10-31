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
import json
import posixpath
import re
import xml.etree.ElementTree as ET
import zipfile

MAX_MEMBER = 64 << 20      # per file inside a zip: guards against zip bombs
MAX_TOTAL = 256 << 20
MAX_IMAGES = 50
MAX_ROWS = 5000            # per sheet or CSV
MIN_IMAGE = 6 << 10        # smaller pictures are icons, bullets and logos
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
        if info.file_size > MAX_MEMBER or self.total + info.file_size > MAX_TOTAL:
            raise ExtractError("The document is too large to read.")
        self.total += info.file_size
        with self.zf.open(info) as f:
            return f.read(MAX_MEMBER + 1)[:MAX_MEMBER]

    def xml(self, name):
        return ET.fromstring(self.read(name))

    def has(self, name):
        return name in self.zf.namelist()

    def add(self, *block):
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
            out[r.get("Id")] = posixpath.normpath(posixpath.join(d, r.get("Target", "")))
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
        elif t not in (q("w:delText"), q("w:instrText"), q("w:pPr"), q("a:pPr")):
            out.append(ooxml_runs(node, pics))
    return "".join(out)


# ---------- Word ----------
def docx_styles(doc):
    """Paragraph style id -> heading level (from the style name or outline level), and -> list numId."""
    heads, lists = {}, {}
    if not doc.has("word/styles.xml"):
        return heads, lists
    for st in doc.xml("word/styles.xml").iter(q("w:style")):
        sid, name = st.get(q("w:styleId")), (st.find(q("w:name")).get(q("w:val")) if st.find(q("w:name")) is not None else "")
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
            if style in heads or (olvl is not None and olvl.get(q("w:val"), "9").isdigit() and int(olvl.get(q("w:val"))) < 6):
                doc.add("h", heads.get(style) or int(olvl.get(q("w:val"))) + 1, text.strip())
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
            for tr in el.iter(q("w:tr")):
                rows.append([" ".join(ooxml_runs(p, pics).strip() for p in tc.iter(q("w:p"))).strip() for tc in tr.findall(q("w:tc"))])
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
                doc.add("table", [[ooxml_runs(tc, []).strip() for tc in tr.findall(q("a:tc"))] for tr in tbl.findall(q("a:tr"))])
        elif t == q("p:pic"):
            blip = el.find(f".//{q('a:blip')}")
            rid = blip.get(q("r:embed")) if blip is not None else None
            if rid in rel:
                doc.image(rel[rid], label)


def pptx(doc):
    for i, name in enumerate(numbered(doc.zf.namelist(), r"ppt/slides/slide\d+\.xml"), 1):
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
    n = 0
    for ch in re.match(r"[A-Z]+", ref).group(0):
        n = n * 26 + ord(ch) - 64
    return n - 1


def xlsx(doc):
    shared = []
    if doc.has("xl/sharedStrings.xml"):
        for si in doc.xml("xl/sharedStrings.xml").iter(q("s:si")):
            shared.append("".join(t.text or "" for t in si.iter(q("s:t"))))
    rel = rels(doc, "xl/workbook.xml")
    for sheet in doc.xml("xl/workbook.xml").iter(q("s:sheet")):
        target = rel.get(sheet.get(q("r:id")))
        if not target or not doc.has(target):
            continue
        rows = []
        for row in doc.xml(target).iter(q("s:row")):
            cells = {}
            for c in row.iter(q("s:c")):
                kind, v = c.get("t"), c.find(q("s:v"))
                if kind == "s" and v is not None and v.text and v.text.isdigit() and int(v.text) < len(shared):
                    val = shared[int(v.text)]
                elif kind == "inlineStr":
                    val = "".join(t.text or "" for t in c.iter(q("s:t")))
                else:
                    val = v.text if v is not None and v.text else ""
                if val and c.get("r"):
                    cells[col_index(c.get("r"))] = val.replace("\t", " ")
            if cells:
                rows.append([cells.get(i, "") for i in range(max(cells) + 1)])
            if len(rows) >= MAX_ROWS:
                break
        if rows:
            doc.add("table", rows)
        doc.flush(sheet.get("name") or "Sheet")


# ---------- OpenDocument ----------
def odf_inline(el):
    """Text of an ODF paragraph, with its spaces, tabs and line breaks (notes and frames left out)."""
    out = [el.text or ""]
    for c in el:
        if c.tag == q("text:s"):
            out.append(" " * int(c.get(q("text:c"), "1")))
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


def odf(doc):
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
            doc.flush(f"Slide {i}")
    elif sheets is not None:
        for t in sheets.iter(q("table:table")):
            rows = []
            for row in t.iter(q("table:table-row")):
                cells = []
                for c in row:
                    if c.tag not in (q("table:table-cell"), q("table:covered-table-cell")):
                        continue
                    val = " ".join("".join(odf_inline(p)) for p in c.iter(q("text:p")))
                    cells += [val] * min(int(c.get(q("table:number-columns-repeated"), "1")), 1 if not val else 50)
                while cells and not cells[-1]:
                    cells.pop()
                if cells:
                    rows.append(cells)
                if len(rows) >= MAX_ROWS:
                    break
            if rows:
                doc.add("table", rows)
            doc.flush(t.get(q("table:name")) or "Sheet")
    else:
        odf_walk(doc, body, "Document", styles)
        doc.flush("Document")


# ---------- HTML and EPUB ----------
class HTMLBlocks(html.parser.HTMLParser):
    """HTML into blocks: headings, paragraphs, (nested) list items, tables and preformatted text."""
    BLOCK = {"p", "div", "section", "article", "blockquote", "header", "footer", "main", "aside", "nav", "figure",
             "figcaption", "dd", "dt", "address", "hr", "title", "body"}
    SKIP = {"script", "style", "head", "noscript", "svg", "template"}

    def __init__(self, doc):
        super().__init__(convert_charrefs=True)
        self.doc, self.buf, self.kind, self.skip, self.lists, self.table, self.row, self.cell, self.pre = doc, [], "p", 0, [], None, None, None, 0

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
        if tag in self.SKIP:
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
            self.emit(); self.table, self.row, self.cell = [], None, None
        elif tag in ("ul", "ol"):
            self.emit(); self.lists.append(tag == "ol")
        elif tag == "li" or tag == "pre" or re.fullmatch(r"h[1-6]", tag):
            self.emit(); self.kind = tag
            self.pre += tag == "pre"
        elif tag in self.BLOCK:
            self.emit()

    def handle_endtag(self, tag):
        if tag in self.SKIP:
            self.skip = max(0, self.skip - 1)
        elif self.skip:
            return
        elif tag in ("td", "th"):
            self.cell = None
        elif tag == "table" and self.table is not None:
            rows = [[re.sub(r"\s+", " ", "".join(c)).strip() for c in r] for r in self.table]
            self.table, self.row, self.cell = None, None, None
            self.doc.add("table", [r for r in rows if r])
        elif tag in ("ul", "ol"):
            self.emit()
            if self.lists:
                self.lists.pop()
        elif tag == "pre":
            self.emit(); self.pre = max(0, self.pre - 1)
        elif tag == "li" or re.fullmatch(r"h[1-6]", tag) or tag in self.BLOCK:
            self.emit()

    def handle_data(self, data):
        if not self.skip:
            (self.cell if self.cell is not None else self.buf).append(data)

    def close(self):
        super().close()
        self.emit()


def html_blocks(doc, raw):
    p = HTMLBlocks(doc)
    p.feed(decode(raw))
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
        path = posixpath.normpath(posixpath.join(base, href)) if href else None
        if path and doc.has(path):
            html_blocks(doc, doc.read(path))
            doc.flush(f"Section {i}")


# ---------- RTF and plain text ----------
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


def decode(raw):
    for enc in ("utf-8-sig", "utf-16") if raw[:2] not in (b"\xff\xfe", b"\xfe\xff") else ("utf-16",):
        try:
            return raw.decode(enc)
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
        text = decode(data)
        try:
            dialect = "excel-tab" if ext == "tsv" else csv.Sniffer().sniff(text[:4096], ",;\t|")
        except csv.Error:
            dialect = "excel"
        doc.add("table", [r for r, _ in zip(csv.reader(io.StringIO(text), dialect), range(MAX_ROWS), strict=False)])
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
