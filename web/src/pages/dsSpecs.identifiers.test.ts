import { expect, it } from "vitest";
import { dsSpecs } from "./Settings";

it.each(["postgres", "mysql", "trino", "snowflake"])("preserves reserved characters in %s path identifiers", (id) => {
  const name = "sales#2026?forecast%";
  const spec = dsSpecs().find((s) => s.id === id)!;
  const url = new URL(spec.build({ host: "db.internal", account: "acme.snowflakecomputing.com", database: name, catalog: name, schema: name, warehouse: "compute" }));
  expect(decodeURIComponent(url.pathname.split("/")[1])).toBe(name);
  expect(url.hash).toBe("");
  if (id === "trino" || id === "snowflake") {
    expect(decodeURIComponent(url.pathname.split("/")[2])).toBe(name);
  }
});
