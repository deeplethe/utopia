"""Domain-neutral structural checks for model-produced TBox deltas.

Semantic class-versus-individual decisions are made by an independent evidence critic.  This
module enforces only ontology invariants and exact structured-value provenance; it deliberately
contains no benchmark- or subject-domain vocabulary.
"""
from __future__ import annotations

import re
import unicodedata

from app.ontology import role_evidence

_CLASS_FIELDS = {
    "classes": ((("label", "name")),),
    "object_properties": (("domain",), ("range",)),
    "data_properties": (("domain",),),
    "subclass_of": (("sub", "child", "subclass"), ("super", "parent", "superclass")),
    "disjoint_with": (("a",), ("b",)),
    "equivalent_class": (("a",), ("b",)),
}

_DATATYPE_ALIASES = {
    "string": "string", "text": "string", "str": "string",
    "integer": "integer", "int": "integer", "long": "integer",
    "float": "decimal", "double": "decimal", "decimal": "decimal", "number": "decimal",
    "boolean": "boolean", "bool": "boolean",
    "date": "date", "datetime": "dateTime", "time": "time",
    "anyuri": "anyURI", "uri": "anyURI", "url": "anyURI",
}
_BARE_DATATYPE_CLASS_NAMES = {
    "string", "integer", "decimal", "boolean", "date", "datetime", "time", "anyuri",
}
_CONTENT_FREE_PROPERTY_NAMES = {"has", "have"}
_XSD_NAMESPACE = "http://www.w3.org/2001/XMLSchema#"


def _normalize(value: str) -> str:
    value = unicodedata.normalize("NFKC", value or "").casefold().replace("_", " ")
    return " ".join(re.findall(r"\w+", value, flags=re.UNICODE))


def _compound_head_mismatch(sub: str, parent: str) -> bool:
    """Detect the high-confidence English ``Node Configuration ⊆ Node`` mistake."""
    sub_tokens = _normalize(sub).split()
    parent_tokens = _normalize(parent).split()
    if len(sub_tokens) <= len(parent_tokens) or not parent_tokens:
        return False
    if not all(any(char.isascii() and char.isalpha() for char in token) for token in sub_tokens):
        return False
    return set(parent_tokens) <= set(sub_tokens) and sub_tokens[-1] != parent_tokens[-1]


def is_lexically_safe_subclass(sub: str, parent: str) -> bool:
    """Return true for high-confidence head inheritance such as ``Admission Plugin ⊑ Plugin``."""
    sub_tokens = _normalize(sub).split()
    parent_tokens = _normalize(parent).split()
    if len(sub_tokens) <= len(parent_tokens) or not parent_tokens:
        return False
    if not all(any(char.isascii() and char.isalpha() for char in token) for token in sub_tokens):
        return False
    return sub_tokens[-len(parent_tokens):] == parent_tokens


def canonical_datatype_name(value: object) -> str | None:
    """Return the canonical XSD local name for a model-emitted datatype token."""
    if not isinstance(value, str):
        return None
    token = unicodedata.normalize("NFKC", value).strip().strip("<>").casefold()
    if token.startswith(_XSD_NAMESPACE.casefold()):
        token = token[len(_XSD_NAMESPACE):]
    elif token.startswith("xsd:"):
        token = token[4:]
    elif "xmlschema#" in token:
        token = token.rsplit("xmlschema#", 1)[-1]
    return _DATATYPE_ALIASES.get(token)


def _is_datatype_class_label(value: str) -> bool:
    token = unicodedata.normalize("NFKC", value).strip().strip("<>").casefold()
    return (
        token.startswith("xsd:")
        or "xmlschema#" in token
        or token in _BARE_DATATYPE_CLASS_NAMES
    ) and canonical_datatype_name(value) is not None


def _first(item: dict, fields: tuple[str, ...]) -> str:
    for field in fields:
        value = item.get(field)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return ""


def _class_references(ontology: dict) -> list[str]:
    references: list[str] = []
    for key, field_groups in _CLASS_FIELDS.items():
        rows = ontology.get(key)
        if not isinstance(rows, list):
            continue
        for row in rows:
            if not isinstance(row, dict):
                continue
            for fields in field_groups:
                if value := _first(row, fields):
                    references.append(value)
    return references


def sanitize_ontology_delta(
    ontology: dict,
    source_text: str,
    existing_class_norms: set[str] | None = None,
    structured_non_type_signals: dict[str, str] | None = None,
    corpus_role_source_text: str | None = None,
    existing_object_property_norms: set[str] | None = None,
    existing_data_property_norms: set[str] | None = None,
) -> tuple[dict, list[dict[str, str]]]:
    """Enforce generic class/property/datatype invariants on a TBox delta.

    Structured scalar values remain blocked until an independent role critic accepts grounded type
    evidence. Semantic paraphrases are handled by that critic; no domain vocabulary is encoded here.
    """
    structured_non_types = role_evidence.structured_non_type_values(source_text)
    existing_norms = {_normalize(label) for label in (existing_class_norms or set())}
    class_rows = ontology.get("classes", [])
    verified_norms = {
        _normalize(_first(row, ("label", "name")))
        for row in (class_rows if isinstance(class_rows, list) else [])
        if isinstance(row, dict)
        and row.get("_role_verified") is True
        and _first(row, ("label", "name"))
    }
    blocked: dict[str, dict[str, str]] = {}
    for label in _class_references(ontology):
        normalized = _normalize(label)
        role_verified = normalized in verified_norms or normalized in existing_norms
        reason = None
        if (
            corpus_role_source_text
            and role_evidence.has_explicit_individual_declaration(corpus_role_source_text, label)
        ):
            # Corpus-wide explicit identity evidence is authoritative even when a local critic
            # accepted the same name as a class.  This prevents merge order from deciding the
            # class/individual boundary across overlapping chunks.
            reason = "exact label is explicitly declared as an instance or individual elsewhere in the corpus"
        if not reason and not role_verified:
            reason = structured_non_types.get(normalized)
        if not reason and not role_verified and structured_non_type_signals:
            reason = structured_non_type_signals.get(normalized)
        if _is_datatype_class_label(label):
            reason = "XML Schema datatype is a literal range, not an OWL class"
        if (
            not reason
            and normalized not in existing_norms
            and not role_evidence.surface_is_grounded(source_text, label)
        ):
            reason = "new class label is not lexically grounded in the source"
        if reason and normalized not in blocked:
            blocked[normalized] = {"label": label, "reason": reason}

    def is_blocked(value: object) -> bool:
        return isinstance(value, str) and _normalize(value) in blocked

    out = dict(ontology)
    classes: list[dict] = []
    rows = ontology.get("classes", [])
    for row in rows if isinstance(rows, list) else []:
        if isinstance(row, dict) and not is_blocked(_first(row, ("label", "name"))):
            cleaned = dict(row)
            cleaned.pop("_role_verified", None)
            classes.append(cleaned)
    available_classes = {
        _normalize(_first(row, ("label", "name")))
        for row in classes
        if _first(row, ("label", "name"))
    }
    available_classes.update(existing_norms)
    out["classes"] = classes

    def unavailable_class(value: object) -> bool:
        return (
            isinstance(value, str)
            and bool(value.strip())
            and _normalize(value) not in available_classes
        )

    existing_object_norms = {
        _normalize(label) for label in (existing_object_property_norms or set())
    }
    existing_data_norms = {
        _normalize(label) for label in (existing_data_property_norms or set())
    }
    incoming_object_rows = ontology.get("object_properties", [])
    object_rows = incoming_object_rows if isinstance(incoming_object_rows, list) else []
    incoming_data_rows = ontology.get("data_properties", [])
    data_rows = incoming_data_rows if isinstance(incoming_data_rows, list) else []
    declared_object_norms = {
        _normalize(_first(row, ("label", "name")))
        for row in object_rows if isinstance(row, dict)
        if _first(row, ("label", "name")) and canonical_datatype_name(row.get("range")) is None
    }
    converted_data_rows: list[dict] = []
    cleaned_object_rows: list[dict] = []
    for row in object_rows:
        if not isinstance(row, dict):
            continue
        cleaned = dict(row)
        label_norm = _normalize(_first(cleaned, ("label", "name")))
        if label_norm in _CONTENT_FREE_PROPERTY_NAMES:
            continue
        datatype = canonical_datatype_name(cleaned.get("range"))
        if datatype:
            if label_norm not in declared_object_norms and label_norm not in existing_object_norms:
                cleaned["range"] = datatype
                if is_blocked(cleaned.get("domain")) or unavailable_class(cleaned.get("domain")):
                    cleaned.pop("domain", None)
                converted_data_rows.append(cleaned)
            continue
        if label_norm in existing_data_norms and label_norm not in existing_object_norms:
            continue
        for slot in ("domain", "range"):
            if is_blocked(cleaned.get(slot)) or unavailable_class(cleaned.get(slot)):
                cleaned.pop(slot, None)
        cleaned_object_rows.append(cleaned)
    out["object_properties"] = cleaned_object_rows

    cleaned_data_rows: list[dict] = []
    rows = list(data_rows)
    rows.extend(converted_data_rows)
    seen_data: set[tuple[str, str, str]] = set()
    for row in rows:
        if not isinstance(row, dict):
            continue
        cleaned = dict(row)
        label_norm = _normalize(_first(cleaned, ("label", "name")))
        if label_norm in _CONTENT_FREE_PROPERTY_NAMES:
            continue
        if label_norm in existing_object_norms and label_norm not in existing_data_norms:
            continue
        if is_blocked(cleaned.get("domain")) or unavailable_class(cleaned.get("domain")):
            cleaned.pop("domain", None)
        raw_range = cleaned.get("range")
        datatype = canonical_datatype_name(raw_range)
        if raw_range not in (None, "") and not datatype:
            continue
        cleaned["range"] = datatype or "string"
        signature = (
            label_norm,
            _normalize(str(cleaned.get("domain") or "")),
            cleaned["range"],
        )
        if signature in seen_data:
            continue
        seen_data.add(signature)
        cleaned_data_rows.append(cleaned)
    out["data_properties"] = cleaned_data_rows

    for key, field_groups in (
        ("subclass_of", (("sub", "child", "subclass"), ("super", "parent", "superclass"))),
        ("disjoint_with", (("a",), ("b",))),
        ("equivalent_class", (("a",), ("b",))),
    ):
        cleaned_rows = []
        rows = ontology.get(key, [])
        for row in rows if isinstance(rows, list) else []:
            if not isinstance(row, dict):
                continue
            if any(is_blocked(_first(row, fields)) for fields in field_groups):
                continue
            endpoints = [_first(row, fields) for fields in field_groups]
            if any(_normalize(endpoint) not in available_classes for endpoint in endpoints):
                continue
            if key == "subclass_of":
                sub = _first(row, ("sub", "child", "subclass"))
                parent = _first(row, ("super", "parent", "superclass"))
                if _compound_head_mismatch(sub, parent):
                    continue
            cleaned_rows.append(dict(row))
        out[key] = cleaned_rows

    return out, list(blocked.values())
