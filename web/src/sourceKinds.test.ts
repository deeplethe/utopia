import { expect, it } from "vitest";
import { hasSyncFields } from "./sourceKinds";

it.each([
  ["github_issues", { repo: " " }, false],
  ["github_repo", { repo: "" }, false],
  ["github_repo", { repo: "https://github.com/o/r" }, false],
  ["github_repo", { repo: " o/r " }, true],
  ["gitlab_repo", { repo: "name" }, false],
  ["gitlab_repo", { repo: "https://gitlab.com/g/p" }, false],
  ["gitlab_repo", { repo: " group/sub/name " }, true],
  ["jira_issues", { jiraUrl: "https://j.test", jiraProject: "" }, false],
  ["jira_issues", { jiraUrl: "https://j.test", jiraProject: "P" }, true],
  ["folder", {}, true],
])("a %s source with %j can sync: %s", (kind, fields, expected) => {
  expect(hasSyncFields(kind, fields)).toBe(expected);
});
