"""Validated, suggestion-only ontology modeling assistant."""
from __future__ import annotations

import json

from app import prompt_config
from app.llm import openrouter
from app.ontology import schema, vocab


_SYSTEM = """You are an ontology modeling assistant working on an existing OWL TBox.
Turn the user's instruction into a small, reviewable structured change set. Never claim that a
change has been applied. Return ONLY one JSON object:
{"summary":"short title","reason":"why these changes fit the request","operations":[...]}

Use at most 20 operations. Allowed operations are add_class, update_class, delete_class,
add_property, update_property, delete_property, add_axiom, delete_axiom, merge_classes,
merge_properties, subordinate_properties, and set_property_union. Existing entities MUST be
referenced by an exact IRI from the supplied ontology. A new class/property may only be introduced
with add_class/add_property and a label, except that merge_properties/subordinate_properties may
use a non-empty target_label for their general object property. Those two operations require exact
existing object-property IRIs in sources and exactly one target (an exact existing object-property
IRI) or target_label. Do not invent IRIs or create implicit classes through axiom or property
references. Prefer the smallest coherent set of edits. Destructive suggestions are allowed but
must be explained because a human will preview and confirm them before application."""

prompt_config.register(
    key="ontology.modeling_assistant",
    category="governance",
    title="Ontology modeling assistant",
    description="Translate a human modeling instruction into a reviewable ontology change set.",
    default=_SYSTEM,
    order=5,
)


_ALLOWED_KEYS: dict[str, set[str]] = {
    "add_class": {"op", "label", "comment"},
    "update_class": {"op", "iri", "label", "comment"},
    "delete_class": {"op", "iri"},
    "add_property": {
        "op", "label", "comment", "kind", "domain", "range", "domain_members", "range_members",
    },
    "update_property": {
        "op", "iri", "label", "comment", "domain", "range", "domain_members", "range_members",
        "clear_domain", "clear_range",
    },
    "delete_property": {"op", "iri"},
    "add_axiom": {"op", "type", "sub", "super", "a", "b"},
    "delete_axiom": {"op", "type", "sub", "super", "a", "b"},
    "merge_classes": {"op", "source", "target"},
    "merge_properties": {"op", "sources", "target", "target_label"},
    "subordinate_properties": {"op", "sources", "target", "target_label"},
    "set_property_union": {"op", "iri", "slot", "members"},
}


class SuggestionError(ValueError):
    pass


def _require_iri(value, allowed: set[str], path: str) -> str:
    if not isinstance(value, str) or value not in allowed:
        raise SuggestionError(f"{path} must be an existing ontology IRI")
    return value


def _class_members(operation: dict, key: str, classes: set[str], path: str) -> None:
    if key not in operation:
        return
    value = operation[key]
    if not isinstance(value, list):
        raise SuggestionError(f"{path}.{key} must be an array")
    for index, iri in enumerate(value):
        _require_iri(iri, classes, f"{path}.{key}[{index}]")


def validate_operations(graph_iri: str, operations) -> list[dict]:
    """Strictly validate model output against the current named entities.

    Importantly, editor operations such as ``add_axiom`` normally accept a label and
    create a missing class for convenience.  AI suggestions are held to a stricter
    boundary: only add_class/add_property may introduce entities, and all references
    elsewhere must be exact existing IRIs.
    """

    if not isinstance(operations, list) or not operations or len(operations) > 20:
        raise SuggestionError("operations must contain between 1 and 20 edits")
    if len(json.dumps(operations, ensure_ascii=False)) > 100_000:
        raise SuggestionError("operations payload is too large")

    view = schema.build_view(graph_iri)
    classes = {item["iri"] for item in view["classes"]}
    object_properties = {item["iri"] for item in view["object_properties"]}
    data_properties = {item["iri"] for item in view["data_properties"]}
    properties = object_properties | data_properties
    clean: list[dict] = []
    for index, operation in enumerate(operations):
        path = f"operations[{index}]"
        if not isinstance(operation, dict):
            raise SuggestionError(f"{path} must be an object")
        name = operation.get("op")
        allowed_keys = _ALLOWED_KEYS.get(name)
        if allowed_keys is None:
            raise SuggestionError(f"{path}.op is not allowed")
        extra = set(operation) - allowed_keys
        if extra:
            raise SuggestionError(f"{path} contains unsupported field(s): {', '.join(sorted(extra))}")

        if name in {"add_class", "add_property"}:
            if not isinstance(operation.get("label"), str) or not operation["label"].strip():
                raise SuggestionError(f"{path}.label is required")
        if name in {"update_class", "delete_class"}:
            _require_iri(operation.get("iri"), classes, f"{path}.iri")
        elif name in {"update_property", "delete_property", "set_property_union"}:
            iri = _require_iri(operation.get("iri"), properties, f"{path}.iri")
            if name == "set_property_union" and operation.get("slot", "range") == "range" and iri in data_properties:
                raise SuggestionError(f"{path} cannot set a class union as a data-property range")
        elif name == "merge_classes":
            _require_iri(operation.get("source"), classes, f"{path}.source")
            _require_iri(operation.get("target"), classes, f"{path}.target")
        elif name in {"merge_properties", "subordinate_properties"}:
            sources = operation.get("sources")
            if not isinstance(sources, list) or not sources or len(sources) > 50:
                raise SuggestionError(f"{path}.sources must contain between 1 and 50 IRIs")
            for source_index, iri in enumerate(sources):
                _require_iri(iri, object_properties, f"{path}.sources[{source_index}]")
            if len(set(sources)) != len(sources):
                raise SuggestionError(f"{path}.sources must contain unique IRIs")
            has_target = operation.get("target") is not None
            has_target_label = operation.get("target_label") is not None
            if has_target == has_target_label:
                raise SuggestionError(
                    f"{path} must contain exactly one of target or target_label"
                )
            if has_target:
                target = _require_iri(
                    operation.get("target"), object_properties, f"{path}.target",
                )
                if target in sources:
                    raise SuggestionError(f"{path}.target cannot also be a source")
            else:
                target_label = operation.get("target_label")
                if not isinstance(target_label, str) or not target_label.strip():
                    raise SuggestionError(f"{path}.target_label must be a non-empty string")

        if name in {"add_property", "update_property"}:
            kind = operation.get("kind", "object") if name == "add_property" else (
                "object" if operation.get("iri") in object_properties else "data"
            )
            if kind not in {"object", "data"}:
                raise SuggestionError(f"{path}.kind must be object or data")
            if operation.get("domain") is not None:
                _require_iri(operation["domain"], classes, f"{path}.domain")
            _class_members(operation, "domain_members", classes, path)
            if kind == "object":
                if operation.get("range") is not None:
                    _require_iri(operation["range"], classes, f"{path}.range")
                _class_members(operation, "range_members", classes, path)
            else:
                if "range_members" in operation:
                    raise SuggestionError(f"{path}.range_members is only valid for object properties")
                if "range" in operation and schema.datatype_node(operation.get("range")).value == vocab.XSD + "string":
                    # ``string`` is valid; unknown datatype names also coerce to string in
                    # the editor, but silently accepting an AI typo is unsafe.
                    raw = str(operation.get("range") or "string").strip()
                    known = {"string", "text", "str", "integer", "int", "long", "float", "double",
                             "decimal", "number", "boolean", "bool", "date", "datetime", "time", "uri", "url"}
                    if raw.lower() not in known:
                        raise SuggestionError(f"{path}.range is not a supported datatype")

        if name in {"add_axiom", "delete_axiom"}:
            axiom_type = operation.get("type")
            if axiom_type == "subclass":
                _require_iri(operation.get("sub"), classes, f"{path}.sub")
                _require_iri(operation.get("super"), classes, f"{path}.super")
            elif axiom_type in {"disjoint", "equivalent"}:
                _require_iri(operation.get("a"), classes, f"{path}.a")
                _require_iri(operation.get("b"), classes, f"{path}.b")
            else:
                raise SuggestionError(f"{path}.type is not allowed")
        if name == "set_property_union":
            if operation.get("slot", "range") not in {"domain", "range"}:
                raise SuggestionError(f"{path}.slot must be domain or range")
            _class_members(operation, "members", classes, path)
            if len(operation.get("members", [])) < 2:
                raise SuggestionError(f"{path}.members needs at least two classes")
        clean.append(dict(operation))
    return clean


def suggest(graph_iri: str, instruction: str) -> dict:
    view = schema.build_view(graph_iri)
    compact = {
        "classes": [
            {key: item.get(key) for key in ("iri", "label", "comment", "superclasses")}
            for item in view["classes"]
        ],
        "object_properties": [
            {key: item.get(key) for key in ("iri", "label", "comment", "domain_members", "range_members")}
            for item in view["object_properties"]
        ],
        "data_properties": [
            {key: item.get(key) for key in ("iri", "label", "comment", "domain_members", "range")}
            for item in view["data_properties"]
        ],
        "axioms": view["axioms"],
    }
    reply = openrouter.chat_sync(
        [
            {"role": "system", "content": prompt_config.get("ontology.modeling_assistant")},
            {"role": "user", "content": (
                "CURRENT ONTOLOGY:\n" + json.dumps(compact, ensure_ascii=False)
                + "\n\nUSER INSTRUCTION:\n" + instruction
            )},
        ],
        temperature=0,
        max_tokens=4000,
    )
    payload = openrouter.extract_json(reply)
    if not isinstance(payload, dict):
        raise SuggestionError("model response must be a JSON object")
    operations = validate_operations(graph_iri, payload.get("operations"))
    summary = str(payload.get("summary") or "Ontology modeling suggestion").strip()[:500]
    reason = str(payload.get("reason") or summary).strip()[:2000]
    return {"summary": summary, "reason": reason, "operations": operations}
