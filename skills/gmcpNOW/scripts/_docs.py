"""Google Docs tab traversal, rendering, and indexed Markdown insertion."""
import re
from _common import Failure


def utf16(text):
    return len(text.encode("utf-16-le")) // 2


def tabs(document):
    result = []
    def visit(items):
        for tab in items:
            result.append(tab)
            visit(tab.get("childTabs", []))
    visit(document.get("tabs", []))
    if not result and "body" in document:
        result = [{"tabProperties": {"tabId": "", "title": "Tab 1"}, "documentTab": document}]
    return result


def choose_tab(document, tab_id=None):
    available = tabs(document)
    if not available:
        raise Failure(3, "The document has no readable tabs.")
    if tab_id is None:
        return available[0]
    for tab in available:
        if tab["tabProperties"]["tabId"] == tab_id:
            return tab
    raise Failure(3, "The requested document tab was not found.", "Read the document to see its tab IDs.")


def tab_range(start, end, tab_id):
    result = {"startIndex": start, "endIndex": end}
    if tab_id:
        result["tabId"] = tab_id
    return result


def paragraphs(content):
    for block in content:
        if "paragraph" in block:
            yield block["paragraph"]
        elif "table" in block:
            for row in block["table"].get("tableRows", []):
                for cell in row.get("tableCells", []):
                    yield from paragraphs(cell.get("content", []))
        elif "tableOfContents" in block:
            yield from paragraphs(block["tableOfContents"].get("content", []))


def anchor_index(tab, anchor, nth=None):
    if not anchor:
        raise Failure(4, "The anchor cannot be empty.")
    matches = []
    # Match across text runs in a paragraph, never across a table-cell boundary.
    for paragraph in paragraphs(tab["documentTab"].get("body", {}).get("content", [])):
        text, ends = "", []
        for element in paragraph.get("elements", []):
            run = element.get("textRun")
            if not run:
                text += "\ufffc"
                ends.append(element.get("endIndex", element.get("startIndex", 0) + 1))
                continue
            position = element["startIndex"]
            for char in run["content"]:
                text += char
                position += utf16(char)
                ends.append(position)
        start = 0
        while (index := text.find(anchor, start)) >= 0:
            matches.append(ends[index + len(anchor) - 1])
            start = index + 1
    if not matches:
        raise Failure(3, "The anchor was not found.", "Re-read the selected tab and choose an exact anchor.")
    if nth is None and len(matches) != 1:
        raise Failure(3, f"The anchor matches {len(matches)} locations.", "Re-read and use --nth (1-based) or a longer anchor.")
    if nth is not None and not 1 <= nth <= len(matches):
        raise Failure(3, f"Anchor occurrence {nth} does not exist; found {len(matches)}.")
    return matches[(nth or 1) - 1]


def end_index(tab):
    content = tab["documentTab"].get("body", {}).get("content", [])
    return max(1, max((item.get("endIndex", 1) for item in content), default=2) - 1)


def markdown_plan(markdown, index=1, tab_id=None, *, separate=False, color="blue"):
    """Parse first, insert once, then apply styles in Google UTF-16 coordinates."""
    from markdown_it import MarkdownIt
    if re.search(r"[\x00-\x08\x0c-\x1f\ue000-\uf8ff]", markdown):
        raise Failure(4, "Markdown contains characters Google strips during insertion.", "Remove control/private-use characters before editing so indices stay correct.")
    tokens = MarkdownIt("commonmark", {"html": False}).parse(markdown)
    output = "\n" if separate else ""
    styles, blocks, lists = [], [], []
    heading = 0
    for token in tokens:
        if token.type in ("bullet_list_open", "ordered_list_open"):
            lists.append("NUMBERED_DIGIT_ALPHA_ROMAN" if token.type == "ordered_list_open" else "BULLET_DISC_CIRCLE_SQUARE")
        elif token.type in ("bullet_list_close", "ordered_list_close"):
            lists.pop()
        elif token.type == "heading_open":
            heading = int(token.tag[1:])
        elif token.type == "heading_close":
            heading = 0
        elif token.type in ("inline", "fence", "code_block"):
            start = index + utf16(output)
            inline, local_styles = inline_text(token.children or []) if token.type == "inline" else (token.content.rstrip("\n"), [])
            if not inline:
                continue
            output += inline + "\n"
            stop = index + utf16(output)
            blocks.append((start, stop, heading, lists[-1] if lists else None, len(lists) - 1))
            for left, right, style in local_styles:
                styles.append((start + left, start + right, style))
        elif token.type == "hr":
            output += "---\n"
    start = index + (1 if separate else 0)
    end = index + utf16(output)
    if end <= start:
        raise Failure(4, "Markdown must contain text.")
    location = {"index": index}
    if tab_id:
        location["tabId"] = tab_id
    full = tab_range(start, end, tab_id)
    requests = [{"insertText": {"location": location, "text": output}},
        {"updateTextStyle": {"range": full, "textStyle": {"bold": False, "italic": False}, "fields": "bold,italic,link"}},
        {"deleteParagraphBullets": {"range": full}},
        {"updateParagraphStyle": {"range": full, "paragraphStyle": {"namedStyleType": "NORMAL_TEXT",
            "indentStart": {"magnitude": 0, "unit": "PT"}, "indentEnd": {"magnitude": 0, "unit": "PT"},
            "indentFirstLine": {"magnitude": 0, "unit": "PT"}}, "fields": "namedStyleType,indentStart,indentEnd,indentFirstLine"}}]
    if color == "blue":
        requests.append({"updateTextStyle": {"range": full,
            "textStyle": {"foregroundColor": {"color": {"rgbColor": {"red": 0.0, "green": 0.2, "blue": 0.8}}}},
            "fields": "foregroundColor"}})
    for left, right, level, bullet, nesting in blocks:
        range_ = tab_range(left, right, tab_id)
        if level:
            requests.append({"updateParagraphStyle": {"range": range_, "paragraphStyle": {
                "namedStyleType": f"HEADING_{level}"}, "fields": "namedStyleType"}})
        if bullet:
            requests.append({"createParagraphBullets": {"range": range_, "bulletPreset": bullet}})
            if nesting > 0:
                requests.append({"updateParagraphStyle": {"range": range_, "paragraphStyle": {
                    "indentStart": {"magnitude": 36 * (nesting + 1), "unit": "PT"},
                    "indentFirstLine": {"magnitude": 36 * nesting + 18, "unit": "PT"}}, "fields": "indentStart,indentFirstLine"}})
    for left, right, style in styles:
        requests.append({"updateTextStyle": {"range": tab_range(left, right, tab_id),
            "textStyle": style, "fields": ",".join(style)}})
    return requests


def inline_text(tokens):
    text, styles, stack = "", [], []
    for token in tokens:
        if token.type in ("strong_open", "em_open", "link_open"):
            style = {"strong_open": {"bold": True}, "em_open": {"italic": True},
                     "link_open": {"link": {"url": token.attrGet("href")}}}[token.type]
            stack.append((utf16(text), style))
        elif token.type in ("strong_close", "em_close", "link_close"):
            start, style = stack.pop()
            if utf16(text) > start:
                styles.append((start, utf16(text), style))
        elif token.type in ("softbreak", "hardbreak"):
            text += "\n"
        elif token.type == "image":
            raise Failure(4, "Image insertion is not supported by this command.", "Remove images or use a deliberate Docs API batch request.")
        else:
            text += token.content
    return text, styles


def render_tab(tab, markdown=True):
    doc = tab["documentTab"]
    lists = doc.get("lists", {})
    def run_text(element):
        run = element.get("textRun")
        if not run:
            return "[embedded object]" if "inlineObjectElement" in element else ""
        value = run.get("content", "")
        if not markdown:
            return value
        # Keep paragraph newlines outside inline style delimiters.
        value, newline = (value[:-1], "\n") if value.endswith("\n") else (value, "")
        style = run.get("textStyle", {})
        if value.strip():
            if style.get("bold"):
                value = "**" + value + "**"
            if style.get("italic"):
                value = "*" + value + "*"
            url = style.get("link", {}).get("url")
            if url:
                value = f"[{value}]({url})"
        return value + newline
    def content_text(content):
        parts = []
        for block in content:
            if "paragraph" in block:
                paragraph = block["paragraph"]
                text = "".join(run_text(e) for e in paragraph.get("elements", []))
                if markdown:
                    kind = paragraph.get("paragraphStyle", {}).get("namedStyleType", "")
                    if kind.startswith("HEADING_"):
                        text = "#" * int(kind[-1]) + " " + text
                    bullet = paragraph.get("bullet")
                    if bullet:
                        level = bullet.get("nestingLevel", 0)
                        levels = lists.get(bullet.get("listId"), {}).get("listProperties", {}).get("nestingLevels", [])
                        glyph = levels[level].get("glyphType", "") if level < len(levels) else ""
                        marker = "1. " if glyph in ("DECIMAL", "ALPHA", "UPPER_ALPHA", "ROMAN", "UPPER_ROMAN", "ZERO_DECIMAL") else "- "
                        text = "  " * level + marker + text
                parts.append(text)
            elif "table" in block:
                rows = [[content_text(cell.get("content", [])).strip() for cell in row.get("tableCells", [])]
                        for row in block["table"].get("tableRows", [])]
                if markdown and rows:
                    width = max(map(len, rows))
                    lines = ["| " + " | ".join(cell.replace("|", "\\|").replace("\n", "<br>") for cell in row + [""] * (width - len(row))) + " |" for row in rows]
                    lines.insert(1, "| " + " | ".join(["---"] * width) + " |")
                    parts.append("\n" + "\n".join(lines) + "\n")
                else:
                    parts.append("\n".join("\t".join(row) for row in rows) + "\n")
            elif "tableOfContents" in block:
                parts.append(content_text(block["tableOfContents"].get("content", [])))
        return "".join(parts)
    return content_text(doc.get("body", {}).get("content", []))
