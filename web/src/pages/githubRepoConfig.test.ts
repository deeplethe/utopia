import { expect, it } from "vitest";
import { githubRepoConfig, globList } from "./githubRepoConfig";

it.each(["notes[,]2026.md", "docs/[a,b]*.md", "docs/[]a,]*.md"])(
  "keeps the comma in the glob character class of %s", (pattern) => {
    expect(globList(`${pattern}, README.md`)).toEqual([pattern, "README.md"]);
  },
);

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

it("a GitLab source keeps its project path under project", () => {
  const saved = { project: "g/p", max_file_bytes: 2000 };
  const sent = githubRepoConfig(saved, { repo: " g/sub/p ", authHeader: "Bearer t", include: "", exclude: "" }, "project");
  expect(JSON.parse(JSON.stringify(sent))).toEqual({ project: "g/sub/p", auth_header: "Bearer t", max_file_bytes: 2000 });
});
