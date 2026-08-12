"""Identity-role hooks shared by ABox resolution and validation.

Identity compatibility is semantic and cannot be inferred safely from class labels. Until roles
are explicitly represented in the ontology, return no cross-class role assertions and let class
hierarchy, disjointness, the independent critic, and human review govern identity decisions.
"""
from __future__ import annotations


def class_role_map(view: dict) -> dict[str, frozenset[str]]:
    return {class_row["iri"]: frozenset() for class_row in view["classes"]}


def roles_for_types(
    class_iris: set[str], roles_by_class: dict[str, frozenset[str]],
) -> frozenset[str]:
    return frozenset(
        role
        for class_iri in class_iris
        for role in roles_by_class.get(class_iri, ())
    )
