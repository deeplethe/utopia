import { expect, it } from "vitest";
import type { Artifact } from "@neverblink/ohi";
import { artifactFilename, ONTOLOGY_UPLOAD_LIMIT, pickArtifact } from "./ohiCatalog";

const file = (
  format: Artifact["format"],
  origin: Artifact["origin"],
  size_bytes = 1000,
): Artifact => ({ format, origin, size_bytes, url: `https://x/${format}`, sha256: "", roots: [] });

it("picks the publisher's own file first, then Turtle", () => {
  const upstreamXml = file("owl-rdfxml", "UPSTREAM");
  expect(pickArtifact([file("owl-turtle", "GENERATED"), upstreamXml])).toEqual({
    artifact: upstreamXml,
  });
  const generatedTtl = file("owl-turtle", "GENERATED");
  expect(pickArtifact([file("owl-rdfxml", "GENERATED"), generatedTtl])).toEqual({
    artifact: generatedTtl,
  });
});

it("skips a file over the upload limit, and reports the smallest when none fits", () => {
  const smaller = file("owl-rdfxml", "GENERATED", ONTOLOGY_UPLOAD_LIMIT);
  expect(
    pickArtifact([file("owl-turtle", "UPSTREAM", ONTOLOGY_UPLOAD_LIMIT + 1), smaller]),
  ).toEqual({ artifact: smaller });
  expect(
    pickArtifact([
      file("owl-turtle", "UPSTREAM", ONTOLOGY_UPLOAD_LIMIT + 2),
      file("owl-rdfxml", "GENERATED", ONTOLOGY_UPLOAD_LIMIT + 1),
    ]),
  ).toEqual({ tooLarge: ONTOLOGY_UPLOAD_LIMIT + 1 });
});

it("offers nothing when the release has no OWL file", () => {
  expect(pickArtifact([file("linkml-yaml", "UPSTREAM"), file("rdfs-turtle", "GENERATED")])).toBeNull();
});

it("names the file after the release, with the suffix of its format", () => {
  const release = { namespace: "fibo", name: "fibo-prod", version: "2019Q4" };
  expect(artifactFilename(release, file("owl-turtle", "UPSTREAM"))).toBe("fibo-fibo-prod@2019Q4.ttl");
  expect(
    artifactFilename({ ...release, version: "1.0 rev/2" }, file("owl-rdfxml", "GENERATED")),
  ).toBe("fibo-fibo-prod@1.0_rev_2.rdf");
});
