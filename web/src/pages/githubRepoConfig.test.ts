import { expect, it } from "vitest";
import { githubRepoConfig } from "./githubRepoConfig";

it("keeps what the edit form does not show, and a cleared glob list falls back to the default", () => {
  const saved = { repo: "o/r", include: ["docs/**"], exclude: ["drafts/**"], max_file_bytes: 2000 };
  const sent = githubRepoConfig(saved, {
    repo: " o/r ",
    authHeader: " ",
    include: "**/*.md, , specs/**",
    exclude: " ",
  });
  expect(JSON.parse(JSON.stringify(sent))).toEqual({
    repo: "o/r",
    include: ["**/*.md", "specs/**"],
    max_file_bytes: 2000,
  });
});
