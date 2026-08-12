"""Domain-neutral evidence helpers for TBox/ABox role decisions.

The model decides semantics; these helpers only enforce provenance and generic structured-data
invariants.  They intentionally contain no ontology-domain vocabulary.
"""
from __future__ import annotations

import re
import unicodedata


ROLE_TYPE = "type"
ROLE_INDIVIDUAL = "individual"
ROLE_LITERAL = "literal"
ROLE_UNCERTAIN = "uncertain"

_TYPE_FIELD_MARKERS = {
    "category", "class", "kind", "type",
    "类别", "分类", "种类", "类型",
}
_FIELD_LINE_RE = re.compile(
    r"(?m)^\s*(?:[-*]\s+)?(?P<key>[^:\n]{1,96})\s*[:：]\s*(?P<value>[^\n]+?)\s*$"
)
_JSON_PAIR_RE = re.compile(
    r'"(?P<key>[^"\\]{1,96})"\s*:\s*(?P<value>"(?:\\.|[^"\\])*"|[-+]?\d+(?:\.\d+)?|true|false|null)',
    re.IGNORECASE,
)
_MARKDOWN_LINK_RE = re.compile(r"\[([^\]]+)\]\([^)]*\)")
_HTML_TAG_RE = re.compile(r"<[^>]+>")
_SHORTCODE_RE = re.compile(r"\{\{[<%]\s*(.*?)\s*[>%]\}\}", re.DOTALL)
_SHORTCODE_TEXT_RE = re.compile(
    r"\btext\s*=\s*(?P<quote>[\"'])(?P<value>.*?)(?P=quote)",
    re.IGNORECASE | re.DOTALL,
)
_LIST_SPLIT_RE = re.compile(r"\s*(?:,|，|;|；|\||→)\s*")
_CJK_RE = re.compile(r"[\u3400-\u9fff\uf900-\ufaff]")


def _shortcode_visible_text(match: re.Match) -> str:
    text_match = _SHORTCODE_TEXT_RE.search(match.group(1))
    return f" {text_match.group('value')} " if text_match else " "


def normalize(value: str) -> str:
    """Normalize text for source-grounding comparisons without translating it."""
    value = unicodedata.normalize("NFKC", value or "").casefold()
    value = _MARKDOWN_LINK_RE.sub(r"\1", value)
    value = _SHORTCODE_RE.sub(_shortcode_visible_text, value)
    value = _HTML_TAG_RE.sub(" ", value)
    value = value.replace("_", " ")
    return " ".join(re.findall(r"\w+", value, flags=re.UNICODE))


def _normalized_phrase_in(source_text: str, phrase: str) -> bool:
    normalized_source = normalize(source_text)
    normalized_phrase = normalize(phrase)
    if not normalized_phrase:
        return False
    if _CJK_RE.search(normalized_phrase):
        return normalized_phrase in normalized_source
    return re.search(
        rf"(?:^| ){re.escape(normalized_phrase)}(?:$| )",
        normalized_source,
    ) is not None


def evidence_is_grounded(source_text: str, evidence: object, *, min_chars: int = 4) -> bool:
    """Return whether an asserted evidence span is actually present in the source."""
    if not isinstance(evidence, str):
        return False
    normalized_evidence = normalize(evidence)
    if len(normalized_evidence.replace(" ", "")) < min_chars:
        return False
    return _normalized_phrase_in(source_text, evidence)


def surface_is_grounded(source_text: str, surface: object) -> bool:
    """Require an individual label to occur in the source rather than be model-invented."""
    if not isinstance(surface, str) or not surface.strip():
        return False
    stripped = surface.strip()
    if re.search(rf"(?<![\w-]){re.escape(stripped)}(?![\w-])", source_text, re.IGNORECASE):
        return True
    return _normalized_phrase_in(source_text, stripped)


def has_explicit_individual_declaration(source_text: str, label: object) -> bool:
    """Return whether the source explicitly names ``label`` as an instance/individual.

    This is deliberately narrower than trying to infer proper names from capitalization.  It only
    recognizes direct identity wording with the candidate as the grammatical subject, so a type
    label in ``X is an instance of Pump`` does not accidentally make ``Pump`` an individual.
    """
    if not isinstance(label, str) or not label.strip():
        return False
    tokens = re.findall(r"\w+", unicodedata.normalize("NFKC", label), flags=re.UNICODE)
    if not tokens:
        return False
    body = r"[\s_-]+".join(re.escape(token) for token in tokens)
    decorated = rf"[`*_]*{body}[`*_]*"
    qname = rf"[`*_]*[A-Za-z][\w-]*:{body}[`*_]*"
    patterns = (
        # A definite, explicitly identified QName: "the `ex:Pump_1` instance".
        rf"\bthe\s+{qname}\s+(?:named\s+)?(?:instance|individual)\b",
        # The exact label is the subject of a direct identity assertion.
        rf"(?<![\w-]){decorated}\s+is\s+(?:an?\s+|the\s+)?(?:named\s+)?(?:instance|individual)\b",
        rf"\b(?:instance|individual)\s+(?:named|called)\s+{decorated}(?![\w-])",
        rf"(?:名为|称为)\s*{decorated}\s*的?(?:实例|个体)",
        rf"该\s*{decorated}\s*(?:实例|个体)",
        rf"{decorated}\s*是\s*(?:一个|该)?\s*(?:实例|个体)",
    )
    return any(re.search(pattern, source_text, re.IGNORECASE) for pattern in patterns)


def _field_is_type_declaration(key: str) -> bool:
    normalized = normalize(key)
    tokens = normalized.split()
    return bool(tokens and (normalized in _TYPE_FIELD_MARKERS or tokens[-1] in _TYPE_FIELD_MARKERS))


def _clean_scalar(value: str) -> str:
    value = value.strip().rstrip(",").strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'`":
        value = value[1:-1]
    return value.strip()


def _scalar_values(value: str) -> list[str]:
    cleaned = _clean_scalar(value)
    if not cleaned or cleaned in {"{}", "[]", "|", ">"}:
        return []
    values = [cleaned]
    bracketed = cleaned[1:-1].strip() if cleaned[:1] in "[(" and cleaned[-1:] in ")]" else cleaned
    if "://" not in bracketed and any(delimiter in bracketed for delimiter in (",", "，", ";", "；", "|", "→")):
        parts = [_clean_scalar(part) for part in _LIST_SPLIT_RE.split(bracketed)]
        values.extend(part for part in parts if part)
    return list(dict.fromkeys(values))


def structured_value_roles(source_text: str) -> dict[str, set[str]]:
    """Map exact structured scalar values to generic source roles.

    Values of explicit ``type``/``kind``/``class``/``category`` fields may denote reusable
    types.  Every other scalar is merely a value at this stage; whether it is an individual or
    a literal is left to the role critic rather than guessed from a domain-specific field name.
    """
    roles: dict[str, set[str]] = {}

    def add(key: str, raw_value: str) -> None:
        role = ROLE_TYPE if _field_is_type_declaration(key) else ROLE_LITERAL
        for value in _scalar_values(raw_value):
            normalized = normalize(value)
            if normalized:
                roles.setdefault(normalized, set()).add(role)

    for match in _FIELD_LINE_RE.finditer(source_text or ""):
        add(match.group("key"), match.group("value"))
    for match in _JSON_PAIR_RE.finditer(source_text or ""):
        add(match.group("key"), match.group("value"))
    return roles


def structured_non_type_values(source_text: str) -> dict[str, str]:
    """Return structured values that have no independent explicit type declaration."""
    return {
        value: "structured scalar value without an explicit type declaration"
        for value, roles in structured_value_roles(source_text).items()
        if ROLE_LITERAL in roles and ROLE_TYPE not in roles
    }
