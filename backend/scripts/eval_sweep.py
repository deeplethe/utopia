"""Run the OntoLearner Text2Onto taxonomy baseline across several ontologies (each in its own
throwaway KS), so we get a multi-ontology picture instead of a single number. Long-running →
launch in the background.
"""
import os
import subprocess
import sys

os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")

RUNS = [
    ("SciKnowOrg/ontolearner-food_and_beverage", "wine"),
    ("SciKnowOrg/ontolearner-units_and_measurements", "owltime"),
    ("SciKnowOrg/ontolearner-units_and_measurements", "uo"),
    ("SciKnowOrg/ontolearner-geography", "geonames"),
]
here = os.path.dirname(__file__)
for repo, name in RUNS:
    print(f"\n{'#' * 20} {name} {'#' * 20}", flush=True)
    subprocess.run([sys.executable, os.path.join(here, "run_ontolearner_baseline.py"), repo, name])
print("\nsweep done.", flush=True)
